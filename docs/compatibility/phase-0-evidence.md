# Phase 0 compatibility evidence

**Snapshot:** 2026-08-29  
**Host:** Windows, `x86_64-pc-windows-msvc`  
**Installed runtime:** Grok Build `1.0.5 (5115b46bc9)` stable  
**Binary SHA-256:** `4B924DAA801663EA20E96382408B1F2B5BA39EFAD62C14D20D88618A9EB0BE64`  
**Client:** `agent-client-protocol` Rust SDK `2.0.0`, stable ACP wire protocol v1

This is observed compatibility evidence, not a claim that Phase 0 is complete. The documented baseline and source links are in [the official-source notes](../research/phase-0-source-notes.md).

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
| Ambient initialize | Confirmed | Advertised `xai.api_key`, `cached_token`, and `grok.com`; the harness selected advertised `cached_token`. |
| Session capabilities | Confirmed | `loadSession`, list, resume, and close were advertised. |
| New/prompt | Confirmed | New session succeeded; no-tools prompt completed with `end_turn`. |
| Turn cancel | Confirmed | A second in-flight prompt received `session/cancel` and completed with `cancelled`. |
| List | Confirmed | Cwd-filtered list succeeded without pagination in the tested state. |
| Load ordering | Partially confirmed | Load succeeded and replay updates were received; exact ordering remains covered deterministically by the fake process and still needs a stored live ordered fixture. |
| Resume | Confirmed | Resume succeeded without a protocol error. |
| Close | Confirmed | Close succeeded. It was not treated as deletion. |
| Standard config/modes | Version-specific gap | `session/new` returned zero standard config options and no legacy mode state. Initialize metadata exposed a `modelState` key, whose value was deliberately not persisted. |
| Standard message/thought updates | Confirmed | Live standard `session/update` traffic included assistant, thought, user, command-list, and session-info variants. |
| Tool events | Confirmed | Two controlled read-only command probes produced standard tool-call and tool-call-update events. |
| Permission callback | Not observed live | Neither a version query nor `git push --dry-run` in this non-Git directory produced `session/request_permission`; no unsafe command or file mutation was attempted to force it. Keep the live exact-scope contract unknown. |
| Elicitation | Confirmed in fake; not observed live | The deterministic fake process exercises form elicitation and a safe cancellation response. Installed-runtime behavior remains unknown. |
| Plans | Confirmed in fake; not observed live | The deterministic fake process emits plan replacement updates. Stable ACP has no approval operation, so Grok plan review/revision semantics remain unknown. |
| Grok extensions | Observed, not normalized | See method inventory below. Payloads were not retained. |
| Stderr | Confirmed in fake and observed live | The fake process proves stderr stays outside ACP framing; authenticated runs emitted 5 lines / 665 bytes whose content was not persisted. Live redaction classification remains to be expanded. |
| Shutdown/process tree | Partial | Each SDK connection returned and reaped its direct child. Windows descendant-orphan behavior has not yet been independently measured. |

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

These methods belong behind the Grok adapter. React must not receive their raw envelopes or payloads.

## Probe side effects

Three compatibility sessions were created, closed, and left in Grok-owned persistence. They were not deleted because close and deletion are different operations and deletion was neither required nor authorized. Two model turns used only controlled compatibility prompts; the command probes were read-only, and this directory was not a Git repository.

## Remaining Phase 0 gates

- Add restart/reconnect and Windows descendant-orphan coverage. The real-child fake lifecycle, permission, elicitation, plan/tool update, malformed-frame, stderr, and crash scenarios are complete.
- Expand the checked-in deterministic fake fixtures with ordered, sanitized installed-runtime fixtures and consistently remapped IDs.
- Add Grok-specific DTOs for the required `_x.ai/*` methods without leaking raw JSON past the adapter.
- Probe configuration/model/reasoning updates and determine whether `session/set_config_option`, legacy methods, or Grok extensions are authoritative in 1.0.5.
- Prove Windows process-tree cleanup, not merely direct-child completion.
- Capture a safe real permission request only if an exact non-destructive reproduction can be established.
