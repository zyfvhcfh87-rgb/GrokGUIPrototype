# Release readiness (Windows-first)

**Issue:** [#19](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/19)

## Packaging

- Product: Grok Build GUI `0.1.0` (`dev.grokbuild.gui`)
- Default bundle target: NSIS current-user installer
- No updater artifacts
- No `resources` or `externalBin` (Grok is never bundled)
- `certificateThumbprint` is null in the repository

Build on Windows:

```powershell
npm ci
npm test
npm run build
cargo test --workspace --locked --no-fail-fast
npm run tauri build -- --bundles nsis
```

The installer is under `target/release/bundle/nsis/`. Debug CI uses `target/debug/bundle/nsis/`.

## Signing and publishing (separate authorization)

Signing credentials must never be committed, logged, or shown in the GUI.

- Windows Authenticode: provide `certificateThumbprint` / `timestampUrl` only in an authorized release environment, or use `signCommand`.
- Do not set `TAURI_SIGNING_PRIVATE_KEY` in this repository.
- Publishing, notarization, and GitHub Releases require an explicit request from Lyn.

Unsigned CI artifacts are for QA only.

## Exit and uninstall

Window close / process exit calls `stop_contained_runtime`, which stops the existing `GrokRuntime` child. On Windows the Job Object still kills descendants if graceful stop times out. Confirm `grok` process count is zero after Quit. Uninstall must not leave a GUI-owned Grok child; Grok's own user install under `%USERPROFILE%\.grok` is supposed to remain.

## Critical packaged flows

Record Grok version and a sanitized capability snapshot (no credentials, paths, or session IDs):

1. Launch → diagnose → Ready
2. Authenticate only through advertised methods
3. Choose workspace → new session → prompt streams
4. Permission/elicitation: deny is first; malformed cannot approve
5. Cancel an active turn
6. Force child exit → Recover → resume without stale generation
7. Quit and relaunch → same workspace/session via ACP
8. Quit with zero leftover Grok processes

## Cross-platform architecture

Linux CI runs `npm test`, `npm run check`, and `cargo test --workspace` without producing an installer. macOS/Linux bundles are not the release target.

## Evidence on this host

Linux cloud agent: frontend/Rust tests and architecture checks. NSIS compile and install/launch require `windows-latest` (workflow `Windows QA`) plus a Windows machine with Grok for live flows. Live installed-Grok permission/elicitation/plan rows stay unknown per the Phase 2 matrix.
