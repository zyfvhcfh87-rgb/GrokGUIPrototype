# Phase 0 official-source notes

**Research snapshot:** 2026-08-29  
**Purpose:** establish the documented contract and the live questions for the Grok Build ACP compatibility spike.  
**Source boundary:** official xAI documentation and source, official Agent Client Protocol documentation/source, and the official Rust SDK pages/source. No blog posts or third-party examples are used.

## Bottom line

The documented happy path is clear: launch `grok agent stdio` directly, exchange newline-delimited JSON-RPC 2.0 over stdin/stdout, initialize protocol v1, authenticate with one of the methods the agent actually advertises, create or restore a session, prompt it, consume `session/update` notifications, and end or cancel the turn cleanly. xAI publishes an end-to-end example of that sequence. ([xAI headless and ACP guide](https://docs.x.ai/build/cli/headless-scripting))

There is nevertheless a real compatibility boundary to prove. At the source snapshot checked here, Grok Build depends on `agent-client-protocol` **0.10.4** with the `unstable` feature, while the current official Rust SDK release is **2.0.0**. The SDK 2.0 changelog says the stable v1 wire schema did not change, but its Rust API did, and Grok also exposes `x.ai/*` extensions. The sensible Phase 0 starting point is therefore the official 2.0.0 client using stable protocol v1, with only the narrowly required feature flags, behind a Grok-specific adapter. It must earn compatibility against the installed binary before the GUI depends on it. ([Grok Build pinned `Cargo.toml`](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/Cargo.toml), [Rust SDK 2.0.0](https://docs.rs/crate/agent-client-protocol/2.0.0), [SDK changelog](https://docs.rs/crate/agent-client-protocol/2.0.0/source/CHANGELOG.md))

Stable ACP v1 does **not** define a plan approve/revise request, does **not** guarantee that a permission request contains an exact shell command/cwd/affected path, and does **not** standardize Grok's model-state or mode extensions. Those are safety-critical discovery items, not details to fill in from assumptions. ([ACP plans](https://agentclientprotocol.com/protocol/v1/agent-plan), [draft plan-operations RFD](https://agentclientprotocol.com/rfds/plan-operations), [ACP tool calls and permissions](https://agentclientprotocol.com/protocol/v1/tool-calls), [ACP extensibility](https://agentclientprotocol.com/protocol/v1/overview#extensibility))

## 1. Grok Build's documented ACP entry point

### Invocation and transport

- The official ACP entry point is `grok agent stdio`. xAI describes it as a JSON-RPC server on stdin/stdout and shows spawning the executable with the argument array `['agent', 'stdio']`, not through a shell. ([xAI headless and ACP guide](https://docs.x.ai/build/cli/headless-scripting#agent-client-protocol-acp))
- The example writes one JSON object followed by `\n`, parses stdout line by line, and reads stderr separately. This agrees with the ACP stdio contract: UTF-8, newline-delimited JSON-RPC, one message per line, stdout reserved for valid ACP messages, and stderr available for diagnostic logs. ([xAI example](https://docs.x.ai/build/cli/headless-scripting#agent-client-protocol-acp), [ACP transports](https://agentclientprotocol.com/protocol/v1/transports#stdio))
- xAI recommends `--no-auto-update` for scripting and ACP integrations so an unattended process does not change versions mid-run. The flag is documented as a common CLI option; Phase 0 should verify its accepted placement with the installed binary instead of constructing a shell command. ([xAI headless guide](https://docs.x.ai/build/cli/headless-scripting#disabling-auto-updates), [xAI CLI reference](https://docs.x.ai/build/cli/reference#common-options))
- The normal headless CLI also supports `-p/--prompt`, `--cwd`, `--session-id`, `--resume`, `--continue`, output-format controls, and `--always-approve`. Those are useful diagnostics, but they are not substitutes for ACP lifecycle calls in the application. ([xAI headless mode](https://docs.x.ai/build/cli/headless-scripting#headless-mode))

### Documented xAI lifecycle

The official JavaScript example performs this sequence: ([xAI ACP example](https://docs.x.ai/build/cli/headless-scripting#agent-client-protocol-acp))

1. Spawn `grok agent stdio` with piped stdin/stdout/stderr.
2. Send `initialize` with `protocolVersion: 1` and client filesystem/terminal capabilities.
3. Inspect the returned `authMethods`; select `xai.api_key` only when it is advertised and the environment is configured, otherwise use an advertised method such as `cached_token`.
4. Send `authenticate`, including xAI's example `_meta.headless` extension.
5. Send `session/new` with an absolute workspace `cwd` and `mcpServers`.
6. Send `session/prompt` with content blocks.
7. Render assistant content from `session/update` notifications while waiting for the `session/prompt` response, which carries completion metadata.
8. Close stdin and wait for the child to exit.

The exact advertised authentication IDs and capabilities are runtime data. `xai.api_key`, `cached_token`, and the example metadata are evidence of current xAI behavior, not values the GUI should hard-code.

### What the current public Grok source says

The public repository was inspected at commit [`bc7f02eddd3d84085849dc19ed216f11c23b0571`](https://github.com/xai-org/grok-build/commit/bc7f02eddd3d84085849dc19ed216f11c23b0571) (2026-08-28). This pinned snapshot is stronger evidence than an unpinned `main` link, but the installed binary can still differ.

- The workspace pins `agent-client-protocol = 0.10.4` with `features = ['unstable']`; its lockfile pairs that with `agent-client-protocol-schema 0.11.4`. ([pinned `Cargo.toml`](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/Cargo.toml), [pinned `Cargo.lock`](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/Cargo.lock))
- Its ACP initializer selects `ProtocolVersion::V1`, advertises load support, always advertises session close, and conditionally advertises list/resume depending on Grok's process-chat mode. ([pinned ACP agent implementation](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs#L438-L475))
- The same response places Grok-specific information such as agent/model state, available commands, MCP data, cancellation rewind, recap, and voice support under `_meta`/`x.ai` extensions. ([pinned ACP initializer metadata](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs#L477-L512))
- The implementation has handlers for session new/load/list/resume/close, prompt, and cancel, but handler presence is not the same as a capability being advertised in every runtime mode. ([pinned session handlers](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs#L918-L954), [pinned cancel handler](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs#L2124-L2132))

**Gap:** public source is not proof of the locally installed executable's version, build flags, runtime mode, or negotiated behavior. Record both `grok --version` and the full sanitized `initialize` response in Phase 0.

## 2. ACP v1 contract the harness should implement

### Initialization and capability discipline

`initialize` is the first protocol request. The client proposes its latest supported integer protocol major and client capabilities; the agent returns the selected protocol, agent capabilities, authentication methods, and optional implementation information. If the agent selects an unsupported protocol, the client should close the connection. Omitted capabilities mean unsupported, and clients are expected to handle capability combinations rather than infer them. ([ACP initialization](https://agentclientprotocol.com/protocol/v1/initialization))

For this project:

- advertise only client behavior the harness actually implements;
- check the selected protocol is v1 before any session request;
- preserve only bounded schema/presence markers for unknown `_meta` data inside the Grok adapter, while discarding its raw values before anything reaches React;
- gate every optional method and UI control on the negotiated capability or returned configuration, not on public-source handler presence.

ACP uses JSON-RPC requests for operations that return a result and notifications for one-way events. Notifications never receive responses. Standard extension methods should be underscore-prefixed, while implementation metadata can travel through `_meta`. ([ACP overview](https://agentclientprotocol.com/protocol/v1/overview))

### Direction and feature matrix

| Direction | Operation | Stable v1 status / gate | Phase 0 expectation |
| --- | --- | --- | --- |
| Client -> agent request | `initialize` | Required before session use | Must succeed and select v1. |
| Client -> agent request | `authenticate` | Use an ID returned in `authMethods` | Exercise at least one safe advertised method available through Grok-owned ambient authentication without reading credential files; retain every other advertised method as unexercised and unknown until separately proved. |
| Client -> agent request | `session/new` | Baseline | Absolute canonical `cwd`; empty MCP list is valid when no servers are supplied. |
| Client -> agent request | `session/prompt` | Baseline | Updates may stream before the final prompt response. |
| Client -> agent notification | `session/cancel` | Baseline prompt-turn cancellation | No response to the notification; original prompt request must ultimately finish as cancelled. |
| Client -> agent request | `session/load` | Top-level `loadSession` capability | Must replay the conversation before returning. |
| Client -> agent request | `session/list` | `sessionCapabilities.list` | Exercise the first page and cwd filter in Phase 0; cursor pagination is a Phase 2 lifecycle permutation. |
| Client -> agent request | `session/resume` | `sessionCapabilities.resume` | Restores context without replaying history. |
| Client -> agent request | `session/close` | `sessionCapabilities.close` | Exercise idle close in Phase 0; close during an active turn is a Phase 2 safety permutation. |
| Client -> agent request | `session/delete` | `sessionCapabilities.delete` | Out of MVP; do not expose unless intentionally added. |
| Client -> agent request | `session/set_config_option` | Config option offered by session | Preferred model/mode/thought-level mechanism. |
| Client -> agent request | `session/set_mode` | Legacy session modes capability | Deprecated compatibility fallback only. |
| Agent -> client notification | `session/update` | Baseline | Normalize known variants; safely retain/ignore unknown variants. |
| Agent -> client request | `session/request_permission` | Client-side baseline method; agent use is optional | Correlate request, tool call, and turn; malformed/orphaned/expired requests default to deny/cancel. |
| Agent -> client request | `elicitation/create` | Client must explicitly advertise the requested mode | Implement only after its safety contract is proven. |
| Agent -> client requests | `fs/*`, `terminal/*` | Individual client capabilities | Do not advertise before narrowly scoped implementations exist. |
| Either side notification | `$/cancel_request` | Optional JSON-RPC request cancellation | Distinct from `session/cancel`; test separately. |

Sources for the table: [ACP overview](https://agentclientprotocol.com/protocol/v1/overview), [session setup](https://agentclientprotocol.com/protocol/v1/session-setup), [session list](https://agentclientprotocol.com/protocol/v1/session-list), [session delete](https://agentclientprotocol.com/protocol/v1/session-delete), [prompt turns](https://agentclientprotocol.com/protocol/v1/prompt-turn), [config options](https://agentclientprotocol.com/protocol/v1/session-config-options), [legacy modes](https://agentclientprotocol.com/protocol/v1/session-modes), and [request cancellation](https://agentclientprotocol.com/protocol/v1/cancellation).

### Session lifecycle semantics

- `session/new` takes an absolute `cwd` and MCP server definitions, then returns a session ID. The session cwd is authoritative even when it differs from the agent process's launch directory. ([ACP session setup](https://agentclientprotocol.com/protocol/v1/session-setup#new-session))
- `session/load` is gated by the top-level `loadSession` capability. It must replay the complete conversation as `session/update` notifications **before** its response. ([ACP load](https://agentclientprotocol.com/protocol/v1/session-setup#load-session))
- `session/resume` is separately gated by `sessionCapabilities.resume`. It restores model context but deliberately does not replay history, so the GUI must already have its own presentation state or obtain history through an explicit load path. ([ACP resume](https://agentclientprotocol.com/protocol/v1/session-setup#resume-session))
- `session/close` is separately gated by `sessionCapabilities.close`. It must cancel ongoing work in the same manner as `session/cancel` and release active resources, but the specification does not promise deletion of persisted history. ([ACP close](https://agentclientprotocol.com/protocol/v1/session-setup#close-session))
- `session/list` is gated by `sessionCapabilities.list`, supports cursor pagination and optional cwd filtering, and returns session ID/cwd plus optional title, update time, and metadata. Agents may later send `session_info_update`. ([ACP session list](https://agentclientprotocol.com/protocol/v1/session-list))
- Additional workspace roots may be sent only when `sessionCapabilities.additionalDirectories` is advertised. ([ACP additional directories](https://agentclientprotocol.com/protocol/v1/session-setup#additional-directories))

**Gap:** ACP leaves some persistence details implementation-defined, including hard versus soft deletion and behavior around deleting active sessions. That deferred operation should stay outside the MVP. ([ACP session delete](https://agentclientprotocol.com/protocol/v1/session-delete))

### Prompt streaming and cancellation

`session/prompt` accepts an array of content blocks constrained by negotiated prompt capabilities. During the request, `session/update` can carry assistant message chunks, thought chunks, plan replacement, tool calls/updates, and usage updates. The eventual response contains a stop reason such as `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, or `cancelled`. ([ACP prompt turns](https://agentclientprotocol.com/protocol/v1/prompt-turn))

Prompt cancellation uses the `session/cancel` **notification**. The client must mark unfinished tool calls cancelled and answer outstanding permission requests as cancelled; the agent should abort promptly and must still resolve the original `session/prompt` request with stop reason `cancelled`. Final updates can legitimately arrive between cancellation and that response, so the reducer must continue accepting correlated updates. ([ACP prompt cancellation](https://agentclientprotocol.com/protocol/v1/prompt-turn#cancellation))

ACP also defines optional JSON-RPC `$/cancel_request`, which targets an individual in-flight request. An implementation may cancel it and nested work, but it must still resolve the original request with a valid partial result or JSON-RPC cancellation error `-32800`. This mechanism is not interchangeable with turn-level `session/cancel`. ([ACP request cancellation](https://agentclientprotocol.com/protocol/v1/cancellation))

**Live questions:** Phase 0 can classify each mechanism from a safe baseline observation or retain it as unknown. Repeated cancellation, late-update timing, and cancellation during permission or elicitation belong to the Phase 2 safety workflow.

### Tools and permission requests

Tools are represented by `tool_call` and `tool_call_update` session updates. A call can expose an ID, title, kind, status, content, file locations, and optional raw input/output; updates may contain only the ID and changed fields. The standard status progression is pending/in-progress to completed/failed. ([ACP tool calls](https://agentclientprotocol.com/protocol/v1/tool-calls))

An agent may ask the client for permission through `session/request_permission`, including the session ID, a tool-call update, and choice options. The client responds with a selected option ID or `cancelled`. Standard option kinds are allow once, allow always, reject once, and reject always. ([ACP permissions](https://agentclientprotocol.com/protocol/v1/tool-calls#requesting-permission))

**Safety gap:** ACP does not require an exact command, cwd, affected path, or persistence scope as dedicated permission fields. Those details might be present in the correlated tool call's title/raw input/locations or in Grok-specific metadata, but the standard alone does not guarantee them. Phase 0 should capture a sanitized installed request only when an exact non-destructive reproduction is available; otherwise installed behavior remains explicitly unknown. Phase 2 owns the exhaustive shell, file-write, network/MCP, malformed, stale, and orphaned permission permutations. If exact scope cannot be established, deny is the safe result.

### Elicitation

Elicitation became stable in the ACP specification on 2026-07-24. A client explicitly advertises support for form and/or URL mode; an empty elicitation capability means no modes are supported. An unsupported mode should fail with invalid params rather than silently downgrade. ([stabilization announcement](https://agentclientprotocol.com/announcements/elicitation-stabilized), [ACP elicitation](https://agentclientprotocol.com/protocol/v1/elicitation))

- **Form mode** uses a restricted, flat JSON-schema subset and is for non-sensitive data.
- **URL mode** is for sensitive/OAuth-style flows. The client should show the full URL and host and obtain explicit consent before opening it.
- The response action is accept, decline, or cancel. URL acceptance means only that the user consented to open the URL, not that the external operation succeeded.
- Completion can be signaled later with `elicitation/complete`; secrets and tokens must not be sent back through ACP or model context.
- Requests are correlated to a session plus a tool call or request ID, so orphan and stale requests need an explicit safe policy.

**Package mismatch to test:** the 2.0.0 Rust crate still places elicitation behind a feature named `unstable_elicitation`, despite the wire feature's stabilization one day after the crate release. This appears to be release timing, not evidence that the protocol feature is still draft. Use the narrow feature only if compilation or the live Grok response requires it. ([SDK 2.0 feature manifest](https://github.com/agentclientprotocol/rust-sdk/blob/v2.0.0/src/agent-client-protocol/Cargo.toml), [elicitation announcement](https://agentclientprotocol.com/announcements/elicitation-stabilized))

### Plans

Stable v1 plans are a one-way `session/update` variant. Each update is a complete replacement list; entries carry content, priority, and status. Plans can change throughout a turn. ([ACP agent plans](https://agentclientprotocol.com/protocol/v1/agent-plan))

There is no stable ACP plan approve, reject, or revise method. The official plan-operations RFD explicitly describes today's plan as having no identifier and every update replacing the prior plan; the proposed operations remain draft and concern finer-grained updates/removal, not a standardized approval workflow. ([plan-operations RFD](https://agentclientprotocol.com/rfds/plan-operations))

**Product consequence:** render observed plan state, but do not invent approval semantics. Probe Grok for `x.ai/*` plan-mode operations and observe whether plan review arrives as a permission, elicitation, command, or extension. Keep that behavior in the Grok adapter.

### Configuration, model, reasoning, and mode

Stable session config options are the preferred mechanism. A session setup response can return a complete `configOptions` list; each option has a current value and type. Standard categories include mode, model, model configuration, and thought level, but categories are presentation hints rather than fixed IDs. Select options are baseline; boolean options require the client to advertise boolean support. Unknown option types should be ignored safely. ([ACP session config options](https://agentclientprotocol.com/protocol/v1/session-config-options), [boolean stabilization](https://agentclientprotocol.com/announcements/boolean-config-option-stabilized))

The client sets one through `session/set_config_option`; the agent returns the **complete** updated config state because one choice can change other available choices. The agent may likewise send a complete `config_option_update`. Legacy modes use `session/set_mode` and `current_mode_update`, but that API is deprecated. ([ACP config updates](https://agentclientprotocol.com/protocol/v1/session-config-options#setting-a-configuration-option), [legacy modes](https://agentclientprotocol.com/protocol/v1/session-modes))

Grok's current source uses the legacy `session/set_model` request for both model and reasoning effort. The request carries the advertised `modelId` plus optional `_meta.reasoningEffort`; the effort must come from the advertised option's `value`, not its label or ID. Current source returns the effective model under `_meta.model` and emits a `model_changed` extension update. ([headless client](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-pager/src/headless.rs#L649-L726), [ACP handler](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs#L2209-L2245), [model switch](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-shell/src/agent/handlers/model_switch.rs#L14-L276))

Grok's session mode IDs are `plan`, `ask`, and `default`, set through standard legacy `session/set_mode`; plan transitions emit standard `current_mode_update`. The private toggle-plan extension is non-idempotent and is not a suitable product control. ([mode IDs](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-shell/src/session/acp_session_impl/session_mode.rs#L4-L23))

Permission policy is a separate, private notification whose canonical values are `default`, `ask`, `auto`, and `always-approve`. It can affect multiple resident sessions when unscoped and has no reliable request/response echo, so Phase 0 deliberately does not mutate it. ([permission mappings](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-pager/src/app/actions.rs#L948-L973), [server scoping](https://github.com/xai-org/grok-build/blob/bc7f02eddd3d84085849dc19ed216f11c23b0571/crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs#L2641-L2717))

Source code names these extensions `x.ai/*`, while Grok's pinned ACP 0.10.4 SDK adds the leading underscore on the JSON-RPC wire. The installed runtime evidence therefore records `_x.ai/*`; the adapter accepts both spellings at its boundary. ([pinned SDK extension serialization](https://github.com/agentclientprotocol/rust-sdk/blob/v0.10.4/src/agent-client-protocol/src/lib.rs#L221-L235))

**Compatibility boundary:** preserve standard config options and Grok metadata without hard-coding labels or model IDs. Installed 1.0.5 can differ from current source even where the method is accepted; its model-setter response is one observed example.

## 3. Official Rust SDK assessment

### Current package and API surface

As checked on 2026-08-29, the official crate release is [`agent-client-protocol` 2.0.0](https://crates.io/crates/agent-client-protocol/2.0.0), published 2026-07-23. The official documentation's quick start constructs an `AcpAgent`, connects a `Client`, sends `InitializeRequest::new(ProtocolVersion::V1)`, and then uses typed session requests. ([official Rust library guide](https://agentclientprotocol.com/libraries/rust), [docs.rs crate page](https://docs.rs/crate/agent-client-protocol/2.0.0), [API docs](https://docs.rs/agent-client-protocol/latest/agent_client_protocol/))

Useful surfaces include:

- `AcpAgent` and `AcpAgentConfig` for launching a child executable with a command path, argument vector, and environment map; ([AcpAgentConfig](https://docs.rs/agent-client-protocol/latest/agent_client_protocol/struct.AcpAgentConfig.html))
- `Client`, `Agent`, `Builder`, `ConnectionTo`, and typed request/notification handlers; ([Builder](https://docs.rs/agent-client-protocol/latest/agent_client_protocol/struct.Builder.html))
- `SessionBuilder` and `ActiveSession` for a higher-level initialized-session workflow; ([session implementation/API](https://docs.rs/agent-client-protocol/latest/src/agent_client_protocol/session.rs.html))
- `Stdio`, line, byte-stream, and channel transports; and
- v1 schema types under `schema::v1`, including explicit session capabilities. ([SessionCapabilities](https://docs.rs/agent-client-protocol/latest/agent_client_protocol/schema/v1/struct.SessionCapabilities.html))

SDK 2.0 changed the Rust API and made draft v2 opt-in, but its changelog states the stable v1 wire schema is unchanged. Stable `ProtocolVersion::LATEST` remains v1 in the schema source. Do not enable the draft `unstable_protocol_v2` feature for this project. ([SDK changelog](https://docs.rs/crate/agent-client-protocol/2.0.0/source/CHANGELOG.md), [protocol version source](https://docs.rs/agent-client-protocol-schema/latest/src/agent_client_protocol_schema/version.rs.html), [2.0 migration guide](https://agentclientprotocol.github.io/rust-sdk/migration_v2.0.html))

The crate's default feature set is empty. Its `unstable` umbrella enables several unrelated capabilities, including auth-method additions, elicitation, usage data, MCP-over-ACP, and session fork. Prefer individual features proved necessary by Grok rather than the umbrella. ([SDK 2.0 feature manifest](https://github.com/agentclientprotocol/rust-sdk/blob/v2.0.0/src/agent-client-protocol/Cargo.toml))

### Process-management facts worth testing

The 2.0.0 `AcpAgent` implementation uses `std::process::Command::new` with separate args/environment and piped stdio, which satisfies the no-shell launch boundary. On Windows it uses `CREATE_NO_WINDOW`; on Unix it creates and can kill a process group. ([pinned SDK process launch](https://github.com/agentclientprotocol/rust-sdk/blob/v2.0.0/src/agent-client-protocol/src/acp_agent.rs#L245-L303))

The implementation bounds retained stderr, gives graceful protocol shutdown a short grace period, and then terminates the child. Its Windows fallback kills the direct child rather than a Unix-style process group. ([pinned SDK termination](https://github.com/agentclientprotocol/rust-sdk/blob/v2.0.0/src/agent-client-protocol/src/acp_agent.rs#L306-L335), [pinned protocol shutdown](https://github.com/agentclientprotocol/rust-sdk/blob/v2.0.0/src/agent-client-protocol/src/acp_agent.rs#L735-L768))

The Phase 0 Windows adapter therefore replaces only the SDK's process-spawn component. `process-wrap` 10.0.0 creates the child suspended, assigns it to a kill-on-close Job Object, and resumes it only after containment succeeds; ACP's public `Lines` and `ConnectTo` continue to provide the official JSON-RPC implementation while the adapter enforces a bounded newline codec and sanitized line observer. Setup failures terminate the suspended child and fail closed rather than falling back to an uncontained launch. ([process-wrap Job Object adapter](https://github.com/watchexec/process-wrap/blob/v10.0.0/src/tokio/job_object.rs#L71-L128), [Win32 job implementation](https://github.com/watchexec/process-wrap/blob/v10.0.0/src/windows.rs#L118-L177), [ACP line transport](https://docs.rs/agent-client-protocol/2.0.0/agent_client_protocol/struct.Lines.html))

Microsoft documents that child processes join the immediate job by default and that `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` terminates job members when the final job handle closes. Creating the process suspended before assignment avoids the race in which it could spawn an uncontained descendant. This boundary is lifecycle containment, not a security sandbox. ([Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects), [AssignProcessToJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject))

The stock SDK behavior remains useful on Unix, but its Windows launcher is not completion evidence:

- verify clean EOF, graceful close, forced termination, app crash, and repeated restart on Windows;
- inspect the process tree to prove no Grok descendant is orphaned;
- keep raw SDK debug callbacks out of persistent application logs because stdin/stdout/stderr can contain prompts, workspace paths, tool payloads, and diagnostics;
- redact before storing fixtures, while preserving message ordering, IDs, shapes, and error behavior.

### Compatibility decision for the spike

Use SDK 2.0.0, stable protocol v1, and a small `GrokRuntime` boundary around:

1. process launch/termination;
2. xAI authentication metadata and advertised method IDs;
3. unknown `_meta` and `x.ai/*` messages;
4. normalized domain events and redacted diagnostics.

Do not pin Grok's older 0.10.4 dependency merely because Grok itself uses it server-side. Client and server crate versions do not need to match when the negotiated wire schema is compatible. Conversely, do not declare 2.0.0 compatible just because both sides say v1. Compile and run the lifecycle below. If typed decoding rejects valid Grok extensions/messages or the SDK cannot meet lifecycle/cleanup requirements, preserve the failing fixture and then choose the smallest evidence-backed fallback: a narrow raw transport boundary or a tested older client version.

Phase 0 places the reusable process, wire-summary, diagnostic, and normalization components inside `grok-runtime`, while the probe crate still owns lifecycle orchestration. Phase 1 assembles those components behind the long-lived `GrokRuntime` interface and exposes the application-facing Tauri command/event boundary.

## 4. Phase 0 classification plan

Every row starts **unknown for the installed binary**. Phase 0 completes the compatibility classification by recording a safe installed observation, deterministic fake coverage, a version-specific or unsupported result, or an explicit unknown. It does not require unsafe or nondeterministic live triggers. Exhaustive safety and lifecycle permutations are Phase 2 work.

| Area | Documented expectation | Evidence to record | Pass condition |
| --- | --- | --- | --- |
| Binary | xAI supports `grok agent stdio` | Resolved canonical path, `grok --version`, file identity; no credentials | Direct arg-array spawn works without shell or auto-update side effects. |
| Framing | UTF-8 NDJSON on stdout; diagnostics on stderr | Raw-to-sanitized ordered frames, stderr classification | No non-ACP stdout; fragmented/combined reads are framed correctly. |
| Initialize | Version negotiation and capabilities first | Full sanitized request/response | Selected v1; unknown extensions produce a safe presence marker without retaining their raw name or payload. |
| Authentication | Choose advertised method only | Advertised methods and result/error for one safe ambient method | No credential-file reads; one safe advertised method succeeds, while every other advertised method remains explicitly unexercised/unknown. |
| New/prompt | Baseline session lifecycle | Session ID shape, updates, prompt result | Markdown chunks, thought, tools, usage, errors normalize deterministically. |
| Cancel | `session/cancel` settles original prompt as cancelled | Baseline notification and final response timeline | One safe installed cancellation settles without a hung request; repeated and callback-in-flight permutations remain Phase 2 work. |
| Request cancel | Optional `$/cancel_request` | Supported/unsupported result and nested effects | Behavior is distinct, correlated, and bounded. |
| List/load | Capability-gated; load replays before response | First-page result plus load update/response ordering | Load replay can rebuild presentation state deterministically; cursor pagination remains a Phase 2 permutation. |
| Resume | Capability-gated; no replay | Resume response and first subsequent prompt | Context resumes without duplicate transcript events. |
| Close | Capability-gated; cancel/release active resources | Idle close and resulting process/session state | Idle close is classified without guessing deletion semantics; active-turn close remains a Phase 2 permutation. |
| Configuration | Complete standard config state or Grok extension | Session response, config changes, resulting updates | Models/reasoning/modes render from negotiated data. |
| Permissions | Correlated request and selectable outcomes | Deterministic fake contract plus a safe installed request only if reproducible | Installed behavior is either observed or explicit unknown; exact-scope and choice permutations are Phase 2 work and otherwise deny/cancel. |
| Elicitation | Explicit form/URL capability and tri-state result | Deterministic fake contract plus safe installed behavior if it occurs | Installed behavior is either observed or explicit unknown; no secret enters persisted diagnostics, and exhaustive modes/actions are Phase 2 work. |
| Plans | Full replacement update; no standard approval | Deterministic replacement fixture plus any safely observed installed update | Replacement semantics are proved without inventing approval; installed review/revision may remain unknown. |
| Tool events | Incremental call/update state | Terminal, file/diff, background task, MCP, failure | Out-of-order/partial updates reduce safely by tool-call ID. |
| Bad input | JSON-RPC errors and transport failure remain bounded | Malformed JSON, invalid params, unknown notification, duplicate/unknown IDs | Harness reports/redacts error and remains usable or reconnects cleanly. |
| Stderr/crash | Stderr is diagnostic; process can exit independently | Bounded redacted stderr, exit status, pending-request outcomes | No secret persistence, deadlock, or orphan process. |
| Restart | Client may reconnect and restore sessions | Crash then respawn/list/load-or-resume trace | State transitions are deterministic and the user can recover. |
| Shutdown | Graceful first, forced fallback | EOF/close timing and process-tree check | Child and descendants are gone; no pending task survives. |

### Cross-phase fixture matrix

The following is the desired end-state matrix, recorded by installed Grok version and negotiated capability fingerprint. Phase 0 captures the safe installed baseline, proves deterministic contracts with the fake agent, and classifies anything unobserved. Phase 2 completes the exhaustive safety and lifecycle permutations in items 5-8, including every permission choice, elicitation mode/action, cancellation timing, cursor pagination, and active-turn close.

1. initialize success, incompatible version, and missing/unknown capability fields;
2. authentication success, rejection, cancellation, and unavailable advertised method;
3. new session plus plain streaming response;
4. assistant/thought/tool/terminal/diff/usage/plan/config/session-info updates;
5. permission allow once, allow always, reject once, reject always, cancelled, malformed, orphaned, and expired;
6. form and URL elicitation for every advertised mode, including decline/cancel and late completion;
7. cancel during text, tool execution, permission, and elicitation;
8. list pagination, load replay ordering, resume without replay, close idle, and close active;
9. invalid JSON, invalid JSON-RPC shape, unknown method/notification, duplicate ID, stderr burst, abrupt EOF, nonzero exit, and restart;
10. graceful shutdown and forced shutdown with a Windows process-tree orphan check.

Sanitization should replace credentials, user text, absolute private paths, repository contents, URLs containing tokens, environment values, and tool output while keeping protocol method names, ordering, IDs (consistently remapped), capability shapes, status transitions, and error codes. Installed-runtime captures use schema-versioned shape wrappers with bounded field paths and an explicit per-frame truncation marker; they discard scalar wire values and raw stderr text.

## 5. Explicit unknowns after documentation research

These questions cannot be closed from official documentation/source alone:

- Which Grok executable/version is currently installed, and whether it matches the public-source snapshot.
- Which auth methods, client callbacks, prompt content types, and session capabilities this build advertises in the selected runtime mode.
- Whether list/resume disappear when Grok's process-chat mode is enabled, and what switches that mode locally.
- Whether the build returns stable `configOptions`, only xAI model-state metadata, or both.
- Which `x.ai/*` methods and update variants are emitted, their ordering guarantees, and whether SDK 2.0 preserves them.
- Whether permission requests always contain enough information to show the exact command, cwd, affected path, and persistence consequence.
- Whether form/URL elicitation is advertised and whether the SDK's narrow elicitation feature is required.
- How Grok represents plan review or revision; stable ACP does not answer this.
- Whether Grok implements `$/cancel_request` in addition to baseline `session/cancel`.
- How malformed frames, invalid request IDs, late responses, stderr bursts, process crashes, and reconnects behave.
- How cursor pagination behaves when an installed session list spans multiple pages, and how installed Grok settles close during an active turn. These remain Phase 2 lifecycle permutations.
- Whether the stock SDK 2.0 direct-child Windows termination path would orphan an installed-Grok-specific descendant. This no longer blocks the product boundary: the Job Object adapter is independently proved with a fake descendant and an installed-runtime crash/restart pass.
- What sensitive data appears on stderr or in raw extension metadata and therefore needs redaction.

The Phase 0 evidence report records which questions were answered and retains the rest as explicit installed-runtime `unknown` entries rather than optimistic defaults. That classification, rather than elimination of every unknown, is the compatibility spike's completion criterion. Phase 2 closes the exhaustive safety and lifecycle permutations required by the product workflow.

## 6. Scope note

No Tauri API is needed for this research-stage harness. Phase 0 remains a minimal Rust compatibility executable so ACP/process behavior can be proved independently of desktop commands, events, permissions, and UI state. Phase 1 assembles the proven `grok-runtime` components into the long-lived `GrokRuntime` service and adds the typed Tauri command/event boundary; restrictive Tauri capabilities and CSP design belong there.
