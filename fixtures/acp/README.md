# Sanitized ACP fixtures

The deterministic fake-agent fixtures are JSONL. Each record is an ordered transcript envelope: `direction` identifies the wire direction and `frame` contains the exact synthetic newline-delimited JSON-RPC value. Scenario-only transport failures use `raw`, `stream`, or `event`, so every checked-in JSONL record remains valid JSON even when it documents an invalid frame or process exit.

The three installed-runtime artifacts are schema-v2 JSON shape wrappers, not raw transcripts. They retain bounded ordered method/direction/kind shapes, consistently remapped request IDs, fixed observations or counters, and field paths capped at 64 per frame. A frame contains `fieldPathsTruncated: true` only when the traversal omitted at least one path. Wire scalar payload values and raw stderr text are discarded.

Synthetic fixture payloads use deterministic placeholders. Across both formats, the checked-in artifacts contain no credentials, user profile paths, private workspace contents, private prompt text, or live URLs.

- `lifecycle.jsonl` covers initialize, authentication, session creation, streamed thought/message/tool/plan updates, permission and elicitation callbacks, cancellation, list, load replay, resume without replay, and close.
- `malformed.jsonl` documents the intentionally invalid stdout frame emitted after the first request.
- `stderr.jsonl` contains a synthetic diagnostic record and proves that stderr remains separate from valid stdout framing.
- `crash.jsonl` documents the deterministic nonzero exit after a request is in flight.
- `installed-grok-1.0.5-initialize-shape.json` preserves two bounded ordered frame shapes from an isolated installed-runtime initialize. Its initialize response hit the field-path cap and carries the truncation marker.
- `installed-grok-1.0.5-controls-shape.json` preserves a selected, renumbered sequence of bounded frame shapes and fixed outcomes from the zero-turn model/reasoning/mode probe. Unrelated MCP, settings, and diagnostic frames are explicitly omitted; none of its retained frames hit the field-path cap.
- `installed-grok-1.0.5-lifecycle-shape.json` preserves 121 bounded ordered frame shapes under `wire.frames` for an authenticated prompt followed by list, load replay, resume, and close. Its capped frame carries the truncation marker; the wrapper stores stderr line/byte counts but never stderr text.
