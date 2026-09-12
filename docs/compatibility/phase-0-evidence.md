# Phase 0 compatibility evidence

**Snapshot:** 2026-08-29  
**Host:** Windows, `x86_64-pc-windows-msvc`  
**Installed runtime:** Grok Build `1.0.5 (5115b46bc9)` stable  
**Binary SHA-256:** `4B924DAA801663EA20E96382408B1F2B5BA39EFAD62C14D20D88618A9EB0BE64`  
**Client:** `agent-client-protocol` Rust SDK `2.0.0`, stable ACP wire protocol v1

This is observed compatibility evidence. The documented baseline and source links are in [the official-source notes](../research/phase-0-source-notes.md). Phase 0 classifies each expected behavior using installed-runtime observations, deterministic fake contracts, version-specific results, unsupported results, or explicit unknowns. It does not turn an unsafe or nondeterministic live trigger into a completion requirement. Exhaustive safety and lifecycle permutations remain Phase 2 work.

## Launch boundary

The harness resolved the canonical installed executable, then launched it directly with this argument array:

```text
["--no-auto-update", "agent", "--no-leader", "stdio"]
```

No shell command string was constructed. The initialize-only probe used a fresh temporary `GROK_HOME`, blank child-process auth variables, disabled updater/crash-handler behavior, and stopped before `authenticate` or `session/new`.

## Installed-runtime results

| Area | Status | Evidence from Grok 1.0.5 |
| --- | --- | --- |
| Resolve/version | Confirmed | Expected user installation resolved; stable version and binary identity recorded. |
| SDK/wire compatibility | Confirmed for tested path | SDK 2.0.0 decoded Grok's ACP v1 initialize and lifecycle traffic. |
| Framing | Confirmed for valid traffic | Zero malformed stdout lines across isolated and authenticated probes. |
| Isolated initialize | Confirmed | Selected protocol v1 with no stderr; advertised auth ID `grok.com`. |
| Ambient authentication | Confirmed for one advertised method | Initialize advertised `xai.api_key`, `cached_token`, and `grok.com`; the harness authenticated successfully with advertised `cached_token`. The other advertised methods were not exercised and remain unknown. |
| Session capabilities | Confirmed | `loadSession`, list, resume, and close were advertised. |
| New/prompt | Confirmed | New session succeeded; no-tools prompt completed with `end_turn`. |
| Turn cancel | Confirmed | A second in-flight prompt received `session/cancel` and completed with `cancelled`. |
| List | Confirmed | Cwd-filtered list succeeded without pagination in the tested state. |
| Load ordering | Confirmed | Load succeeded and the checked-in installed-runtime fixture preserves replay updates between the `session/load` request and its correlated response. The deterministic fake covers the same ordering contract. |
| Resume | Confirmed | Resume succeeded without a protocol error. The installed shape showed session/config state updates but no assistant, user, or thought replay between request and response; the deterministic fake now asserts the same no-transcript-replay contract. |
| Close | Confirmed while idle | Close succeeded after the tested work settled. It was not treated as deletion; close during an active turn remains a Phase 2 safety permutation. |
| Ordered installed fixtures | Confirmed for tested paths | The checked-in schema-v2 shape wrappers preserve isolated initialize, the selected zero-turn control sequence, and the authenticated prompt/list/load/resume/close sequence with consistently remapped IDs and no wire scalar payload values. Per-frame field paths are capped at 64; `fieldPathsTruncated: true` appears only when traversal omitted one or more paths. The lifecycle wrapper contains 121 ordered frames with zero malformed or dropped transport frames. |
| Model/reasoning control | Confirmed, version-specific | A disposable zero-turn session was created with the initialize-advertised model/effort metadata and then accepted those same values through `session/set_model`. The setter response confirmed the effective model and emitted `_x.ai/session_notification` model-change shapes. Installed 1.0.5 encodes the response as `_meta.model.Ok`; current source documents `_meta.model` as a string, and the adapter handles both. |
| Session mode control | Confirmed, unadvertised | The same disposable session accepted `session/set_mode` to `plan`, emitted standard `current_mode_update`, accepted restoration to `default`, and then closed. Because 1.0.5 advertised neither modes nor standard config options, this is compatibility evidence—not permission to hard-code those controls in the product. |
| Standard config options | Version-specific gap | `session/new` returned zero standard config options. `session/set_config_option` therefore remained capability-gated and was not sent. |
| Grok session-response metadata | Typed normalizer implemented and tested | The runtime-boundary normalizer projects new/load/resume response envelopes into bounded model catalogs and legacy `_meta.x.ai/sessionConfig.options`. Workspace paths, session detail, opaque values, credentials, and arbitrary object keys are dropped. It is not yet wired into the probe's live orchestration; standard ACP config options/modes remain on the SDK path. |
| Permission policy mutation | Intentionally not probed | Private `_x.ai/yolo_mode_changed` is fire-and-forget and can affect resident sessions globally. The control probe records an explicit skip and never sends it. |
| Standard message/thought updates | Confirmed | Live standard `session/update` traffic included assistant, thought, user, command-list, and session-info variants. |
| Tool events | Confirmed | Two controlled read-only command probes produced standard tool-call and tool-call-update events. |
| Permission callback | Not observed live | Neither a version query nor `git push --dry-run` in this non-Git directory produced `session/request_permission`; no unsafe command or file mutation was attempted to force it. Keep the live exact-scope contract unknown. |
| Elicitation | Confirmed in fake; not observed live | The deterministic fake process exercises form elicitation with accept and cancel responses. Installed-runtime behavior remains unknown. |
| Plans | Confirmed in fake; not observed live | The deterministic fake process emits plan replacement updates. Stable ACP has no approval operation, so Grok plan review/revision semantics remain unknown. |
| Grok extensions | Typed normalizer implemented and tested for observed set | The runtime-boundary adapter has strict typed paths for all 12 observed `_x.ai/*` methods, accepts SDK and wire spellings, drops unknown payloads, and retains only bounded safe key names for malformed known payloads. Live probe orchestration does not yet consume these normalized outcomes. |
| Stderr | Confirmed in fake and observed live | The fake process proves stderr stays outside ACP framing. The recorded authenticated lifecycle run emitted 5 stderr lines / 665 bytes; its installed shape wrapper retains only those counts, never the text. The managed adapter likewise retains only bounded line/byte counters. |
| Restart/reconnect | Confirmed for fake and installed runtime | The deterministic fake persists a synthetic session across a post-`session/new` crash, then a fresh process initializes, authenticates, lists, resumes without transcript replay, loads with replay, and closes it. Against installed Grok, a Job Object-contained process initialized, authenticated, created a zero-turn session, and was deliberately aborted and awaited; a fresh contained process found that exact hidden ID in the cwd-filtered list, resumed it, and closed it. That managed-restart pass scanned nine sessions on one page at the time. The product's automatic supervision state machine remains Phase 1 work. |
| Shutdown/process tree | Implemented for every Windows ACP stdio probe path; installed proof covers the dedicated managed paths plus ordinary initialize and controls | Graceful EOF reaps the fake process and its sentinel descendant. An independent topology test proves that killing only the fake parent leaves its sentinel descendant alive on Windows; source inspection shows the stock SDK's Windows fallback likewise kills only its direct child. `WindowsAcpProcess` closes that gap by creating the child suspended, assigning a kill-on-close Job Object, and only then resuming it. Both graceful and forced branches passed ten consecutive final stress runs. The implementation routes ordinary initialize/lifecycle/control probes through this adapter. Dedicated managed initialize and restart passed against installed Grok, as did a final ordinary initialize and zero-turn control run through the shared adapter; the ordinary runs reported zero malformed stdout frames, dropped frames, and stderr lines. Exact post-run `grok` and `fake-acp-agent` process counts were both zero. The authenticated prompt lifecycle was not rerun solely to re-prove launcher selection after routing. |

## Sanitized capability snapshot

The installed build advertised this standard capability shape in both isolated and authenticated runs:

```json
{
  "protocolVersion": 1,
  "agentCapabilities": {
    "loadSession": true,
    "promptCapabilities": {
      "image": false,
      "audio": false,
      "embeddedContext": true
    },
    "mcpCapabilities": {
      "http": true,
      "sse": true
    },
    "sessionCapabilities": {
      "list": {},
      "resume": {},
      "close": {}
    },
    "auth": {}
  }
}
```

The response also contained these metadata keys. Values were intentionally discarded:

```text
grokShell
defaultAuthMethodId
x.ai/mcp/sdk
x.ai/pluginDirs
currentWorkingDirectory
agentVersion
agentId
agentInstanceId
hostname
modelState
mcpServers
mcpApps
metadata
availableCommands
cancelRewind
sessionRecap
voiceMode
```

## Observed extension method inventory

Only method names were retained:

```text
_x.ai/announcements/update
_x.ai/mcp/init_progress
_x.ai/mcp/server_status
_x.ai/mcp/servers_updated
_x.ai/mcp_initialized
_x.ai/models/update
_x.ai/queue/changed
_x.ai/session/prompt_complete
_x.ai/session/update
_x.ai/session_notification
_x.ai/sessions/changed
_x.ai/settings/update
```

These methods belong behind the Grok adapter. `grok-runtime` now provides tested normalizers for typed model, settings, session, queue, MCP, announcement, usage, and status outcomes. Unknown extension names become a fixed marker, unknown payloads are discarded, and React must not receive raw envelopes or payloads. Session-response metadata such as `_meta.x.ai/sessionConfig.options` is handled by a separate typed response normalizer rather than widening the notification adapter. Phase 1 will assemble these components with lifecycle orchestration behind the long-lived `GrokRuntime` interface.

## Probe side effects

Eleven compatibility sessions were created, closed, and left in Grok-owned persistence across the Phase 0 runs. They were not deleted because close and deletion are different operations and deletion was neither required nor authorized. Six model turns used only controlled compatibility prompts. Three control sessions and two crash/restart sessions were zero-turn: the controls repeated already-advertised model/effort values, restored mode to `default`, and did not touch global permission policy; the restart probes only exercised process and session lifecycle. Terminal commands requested during the controlled model turns were read-only.

## Installed-runtime unknowns retained after Phase 0

- Installed authentication is confirmed only for the advertised ambient `cached_token` method. The advertised `xai.api_key` and `grok.com` methods were not exercised and remain unknown.
- A safe real permission request should be captured only if an exact non-destructive reproduction can be established; otherwise keep the installed exact-scope contract unknown and deny malformed or insufficient requests.
- Installed-runtime elicitation and plan-update behavior remain unknown because no safe deterministic trigger was established. The fake covers permission allow-once/reject-once, form elicitation accept/cancel, plan replacement, and text cancellation. The remaining permission, elicitation, and cancellation permutations belong to the Phase 2 safety workflow.
- The tested installed session list fit on one page, and close was exercised only while idle. Cursor pagination and close during an active turn remain Phase 2 lifecycle permutations.
- Grok behavior for protocol-level `$/cancel_request` remains unknown. Phase 0 validates its exact spelling, framing, and bounded correlation only; it does not claim an installed-runtime behavior probe.

Phase 2 classified these remaining items in [phase-2-safety-matrix.md](phase-2-safety-matrix.md) without rewriting the observations above.
