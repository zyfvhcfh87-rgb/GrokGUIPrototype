# Phase 1 restart, resume, and vertical-slice evidence

**Issue:** [#12](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/12)  
**Host used for automated coverage in this change:** macOS (Windows-gated fake-adapter tests skipped here)  
**Packaged Windows development-build walkthrough:** still required for the issue’s first acceptance criterion

This note records how the Phase 1 path is implemented and what has been proven automatically. It does not copy credentials, Grok session files, workspace contents, or stderr text.

## Path

```text
launch -> diagnose/authenticate -> select workspace -> new session
       -> send prompt -> stream response/tool event -> restart app -> resume session
```

Sibling recovery path:

```text
child exit -> runtime_failed diagnostic -> Recover (runtime_restart)
           -> new event generation -> list -> session_resume
```

App restart restores through GUI preferences plus ACP:

1. `recent-workspaces.json` version 2 stores canonical recent paths and an optional last session identifier per workspace.
2. After the runtime is `ready`, the setup controller opens the first **available** recent workspace.
3. The session controller lists that workspace through ACP and resumes the remembered identifier if it is still listed.
4. Grok session files are never read.

Process failure restores through the same ACP resume after `runtime_restart`. A newer `generation` clears live conversation state before that resume. Late events from the previous generation are ignored.

## Automated coverage

| Area | Status | Evidence |
| --- | --- | --- |
| Preference v1 migrate / v2 last session | Implemented | `src-tauri/src/workspace.rs` unit tests: v1 files load, remembering a session persists version 2, invalid session IDs are dropped, close forgets, remove drops the mapping. |
| Contract field `lastSessionId` | Implemented | Shared manifest, TypeScript DTO fields, and Rust `assert_fields` for `recentWorkspace`. |
| Launch workspace restore | Implemented | Setup controller test opens the first available recent and skips an unavailable first entry. |
| Launch session restore | Implemented | Session controller resumes `lastSessionId` after list; a missing identifier does not resume another session. |
| Recover uses `runtime_restart` | Implemented | Setup controller retry on `failed` calls `restartRuntime`. |
| Generation-safe re-resume | Implemented | Session controller generation bump clears live selection, re-lists, and resumes the same ID. Conversation reducer already drops older generations. |
| Stale events after recover | Implemented | `tests/phase-1-restore.test.mjs` streams user/tool/assistant cards, restarts, delivers a generation-1 assistant chunk, and asserts it is absent while a generation-2 chunk appears. |
| Fake-adapter crash then resume | Implemented, Windows-gated | `a_crashed_runtime_reports_failure_and_can_restart_cleanly` now lists **and resumes** the persisted session after `restart()`. Existing Job Object tests still cover descendant cleanup. |
| Duplicate runtimes | Unchanged | One Tauri-managed `GrokRuntime`, lifecycle mutex, idempotent start. |
| Packaged Windows development build | Not proven in this worktree | Checklist below. |

Sanitized capability snapshot for a live Windows run should reuse the Phase 0 shape (ACP v1, `loadSession`, list/resume/close) without recording auth material or workspace paths. Until that run exists, treat the Phase 0 snapshot in [phase-0-evidence.md](phase-0-evidence.md#sanitized-capability-snapshot) as the last installed-runtime record.

## Windows packaged-build checklist

Run from a Windows development checkout with Grok Build installed and ambient authentication already available to Grok (never through this GUI):

```powershell
npm test
npm run build
cargo test --workspace --all-targets
npm run tauri build -- --no-bundle
npm run tauri dev
```

Then walk:

1. Launch diagnoses Grok and reaches Ready.
2. Choose a workspace; create a session; send a prompt; observe streamed Markdown and a tool card if the runtime emits one.
3. Quit the app fully. Relaunch. Confirm the same workspace is selected and the same session is resumed through ACP (no session-file access).
4. Force the Grok child to exit. Confirm a recoverable diagnostic and Recover. Confirm the same session resumes and no generation-1 events appear as new cards.
5. Quit again and confirm process counts for `grok` and `fake-acp-agent` are zero.

Record the installed Grok version, negotiated capability snapshot (no credentials), and those process counts in this file when the walkthrough is done.

## Remaining unknowns

- Live packaged Windows proof of the path above.
- Installed-runtime permission, elicitation, and plan-review behavior remain Phase 2, as in Phase 0.
- This change does not add automatic crash-loop supervision. Recover is a single user-triggered restart.
