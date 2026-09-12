# Phase 2 safety matrix

**Issue:** [#16](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/16)  
**Recorded:** 2026-09-12  
**This environment:** Linux cloud agent, no installed Grok binary  
**Last live installed snapshot:** [Phase 0](phase-0-evidence.md) on Windows, Grok Build `1.0.5 (5115b46bc9)` stable

This is the Phase 2 successor to the Phase 0 installed-runtime unknowns. It does not rewrite that evidence. Each row is classified as confirmed, unsupported, version-specific, or explicitly unknown. No unsafe side effect was performed merely to close an unknown. This host cannot produce new installed-Grok observations.

Permission and elicitation UI contracts remain those recorded in [phase-2-interactions.md](phase-2-interactions.md). Plan review and turn cancel shipped with [#14](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/14). Workspace changes, diagnostics, and recovery shipped with [#15](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/15).

## Matrix

| Row | Classification | Live installed Grok | Deterministic fake / UI | Notes |
| --- | --- | --- | --- | --- |
| Advertised `xai.api_key` and `grok.com` | Explicitly unknown | Not exercised. Phase 0 confirmed only ambient `cached_token`. | GUI lists whatever initialize advertises. No credential contents are read, copied, or logged. | Do not live-exercise these methods to close the unknown. |
| Exact installed permission-request scope | Explicitly unknown | Phase 0 found no safe non-destructive reproduction of `session/request_permission`. This host has no Grok binary. | Fake allow-once / reject-once / cancel. UI contract tests cover advertised decisions, malformed scopes, and deny-by-default. | Keep deny as the safe default for malformed, orphaned, expired, or unsupported requests. |
| Installed elicitation | Explicitly unknown | No safe live trigger in Phase 0. | Fake form accept/cancel. UI contract tests cover labelled controls, cancel-before-accept, and unsupported forms. | |
| Installed plan-update | Explicitly unknown | No live plan update in Phase 0. Stable ACP has no approval RPC. | Fake emits plan replacement. Advertised `/approve_plan` and `/revise_plan` are exercised through the conversation controller. | Product never invents plan commands the runtime did not advertise. |
| Protocol-level `$/cancel_request` | Explicitly unknown (installed); unsupported as a product command | Phase 0 confirmed spelling, framing, and bounded correlation only. Installed settlement behavior remains unknown. | Fake JSON-RPC: a matching `$/cancel_request` notification or request resolves a hanging `session/prompt` with `stopReason: cancelled`, not JSON-RPC `-32800`. | `GrokRuntime` turn cancel is `session/cancel`. There is no public `RuntimeCommand` for `$/cancel_request`. |
| Multi-page session listing | Confirmed in fake and UI; explicitly unknown on installed Grok | Phase 0 list fit on one page. | Fake `--paginate-sessions` returns `session-001` then `session-002` with cursor `fixture-page-2`. `GrokRuntime` forwards the cursor. The session list offers Load more, appends unique workspace-scoped rows, and fail-closes repeated cursors. | |
| Close during an active turn | Confirmed in fake, runtime, and UI; explicitly unknown on installed Grok | Phase 0 closed only while idle. | Fake `session/cancel` or `session/close` settles a hanging prompt as `cancelled`. `GrokRuntime` sends `session/cancel` then `session/close`. The conversation controller clears `sending` on `closed` before the prompt promise settles. | Process loss during a hung turn is a separate crash fixture; it must fail the prompt rather than report success. |

## Product invariants recorded here

- Spawn Grok and Git with argument arrays. Never construct a shell command string.
- Do not parse, copy, log, or expose `~/.grok/auth.json`.
- React does not know JSON-RPC method names, request IDs, framing, credential locations, executable arguments, PIDs, or stderr text.
- `$/cancel_request` stays at the fake JSON-RPC contract. The application cancel path remains session-level `session/cancel`.
- Workspace Git status is not attributable to the current session.

## What this environment did not prove

- Any new live Grok 1.0.5 (or later) permission, elicitation, plan, pagination, or active-close observation.
- Packaged Windows desktop QA of the safety workflow (Phase 3 / #19).
- Installed `$/cancel_request` settlement, including whether Grok uses `-32800` or a partial prompt result.

## Verification recorded with this successor

Commands for this Linux checkout (no Grok binary):

```text
cargo fmt --all
cargo test -p fake-acp-agent --locked --no-fail-fast
cargo test -p grok-runtime --locked --no-fail-fast
NODE_OPTIONS='--experimental-strip-types' npm test
npm run check
```

Sanitized capability snapshot for live Windows remains the Phase 0 initialize shape: ACP v1, `loadSession`, list/resume/close, advertised auth IDs `xai.api_key`, `cached_token`, and `grok.com`. No credentials or private workspace contents are recorded here.
