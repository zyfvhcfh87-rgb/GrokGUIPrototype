# Sanitized ACP fixtures

Each JSONL record is an ordered transcript envelope. `direction` identifies the wire direction and `frame` is the exact newline-delimited JSON-RPC value. Scenario-only transport failures use `raw`, `stream`, or `event` so every checked-in fixture remains valid JSONL even when it documents invalid JSON or a process exit.

All identifiers, text, timestamps, commands, and Windows paths are deterministic placeholders. These fixtures contain no credentials, user profile paths, private workspace contents, or live URLs.

- `lifecycle.jsonl` covers initialize, authentication, session creation, streamed thought/message/tool/plan updates, permission and elicitation callbacks, cancellation, list, load replay, resume ordering, and close.
- `malformed.jsonl` documents the intentionally invalid stdout frame emitted after the first request.
- `stderr.jsonl` documents a diagnostic that must remain separate from valid stdout framing.
- `crash.jsonl` documents the deterministic nonzero exit after a request is in flight.
