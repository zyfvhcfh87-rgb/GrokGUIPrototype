# GrokRuntime service boundary

`GrokRuntime` is the single long-lived application service between Tauri and Grok Build. It assembles the Phase 0 process adapter, ACP transport, safe extension normalizers, and recovery behavior behind application-domain commands and events.

```text
React
  <-> narrow Tauri commands + `grok-application-event`
one Tauri-managed `GrokRuntime`
  <-> private worker actor + normalized state
Windows Job Object-contained ACP process
  <-> `grok agent stdio`
```

## Public boundary

The native application core can discover/start/stop/restart the runtime, read its snapshot, execute a `RuntimeCommand`, subscribe to `RuntimeEvent`, and answer a GUI-owned permission or elicitation interaction ID. Tauri maps that internal interface to operation-specific application commands; the generic enums are not React inputs. The reviewed application contract is documented in [the application contract note](application-contract.md).

Responses and events contain bounded application data: negotiated capabilities, sanitized session summaries, streamed text and thought chunks, tool state, exact validated permission scope, elicitation forms, plans, usage supplied by the runtime, lifecycle state, safe standard metadata, normalized `x.ai/*` extension updates, and redacted diagnostics.

The boundary does not expose ACP method names for supported operations, JSON-RPC IDs, raw envelopes, transport framing, Grok credential contents, the resolved executable path, launch arguments, stderr text, process IDs, or native process handles. Unknown or malformed extension payloads become fixed, payload-free observation markers.

## Ownership and lifecycle

One `GrokRuntime` instance is managed by Tauri for the application lifetime. A serialized lifecycle gate makes repeated start and stop operations idempotent. Restart stops the current worker before constructing a fresh connection. The worker monitors connection completion and publishes `failed` or `disconnected` state as appropriate. Event delivery is bounded; if the desktop consumer falls behind, the Tauri bridge emits an explicit recoverable failure instead of silently skipping the lag condition.

Normal stop cancels active prompts and interactions, closes the command loop, and gives the contained child a bounded graceful-shutdown window. If that window expires, aborting the worker drops the Windows Job Object and terminates the process tree. Dropping the last service owner closes its command channel, which also releases the worker and contained child. Permission and elicitation shapes that cannot be represented exactly are declined automatically; the GUI never receives an ambiguous request it could accidentally approve.

## Test seam

`RuntimeTestTarget` is available only through the crate's `test-support` feature. Contract tests launch `fake-acp-agent` across the same worker and process boundary used in production. They cover idempotent lifecycle, session recovery, streaming events, permission and elicitation responses, advertised controls, cancellation, crash/restart/resume recovery, bounded shutdown, and descendant cleanup. The opt-in probe remains the compatibility adapter for the locally installed Grok runtime. The desktop Recover path uses the same `restart()` seam and then resumes through ACP; it does not auto-loop a crashed child.
