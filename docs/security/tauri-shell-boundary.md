# Tauri shell security boundary

Issue #6 establishes a deliberately small desktop boundary, issue #7 adds the long-lived runtime, issue #8 narrows the React-facing interface to reviewed commands and bounded application events, and issue #9 adds setup and workspace selection behind that same boundary. The shell renders the packaged React application and grants the webview no filesystem, process, dialog, external-link, network, or broad plugin permissions.

## Trust boundary

```text
packaged React assets
    -> one local Tauri webview (`main`)
    -> event listen/unlisten capability only
    -> reviewed application DTOs and `grok-application-event`
    -> private `GrokRuntime` command and event enums
    -> no native access plugins
```

- Production content comes only from `../dist`; the main window URL is the local `index.html` entry point.
- Development uses the loopback-only Vite server at `127.0.0.1:1420`. It is the sole development CSP exception and is not present in the production CSP.
- Vite excludes `src-tauri` and the shared Cargo `target` directory from its watcher so Rust rebuilds cannot trigger frontend reloads or collide with locked Windows build artifacts.
- `withGlobalTauri` and the asset protocol are disabled.
- No remote origin is attached to a capability. The `main-shell` capability is local, targets only the `main` window, and grants only `core:event:allow-listen` and `core:event:allow-unlisten` for the normalized application event channel.
- Tauri's compile-time CSP rewriting remains enabled, and the production policy permits no remote script, style, font, frame, form, object, image, or network source.
- The Rust crate registers only setup/workspace, lifecycle, session, turn/control, and interaction-response operations; it initializes no plugins. The generic runtime command enum is not a Tauri input. The bridge accepts no raw JSON-RPC payload, executable path, launch argument, credential, native handle, or unrestricted filesystem operation.
- Runtime events are projected to bounded application DTOs and wrapped in a generation plus sequence before emission. Unknown extension method names and payloads are discarded rather than forwarded.
- Tauri application commands are available to local application windows by default. This application defines only the packaged `main` window and attaches no remote origin. Adding another window or any remote content requires a new boundary review before it may reach these commands.
- The service owns the contained Grok child process. React cannot spawn arbitrary processes or access Grok credential and session files. The native folder dialog is created by the reviewed `workspace_pick` Rust command rather than a general dialog or filesystem plugin; selected paths are canonicalized and validated before they cross the application seam. External links open only through `open_external_url`, which accepts credential-free `http` and `https` URLs and rejects every other scheme before spawning the platform opener.
- Recent workspace preferences are capped, versioned, and limited to canonical directory paths. Missing entries remain visibly stale until the user removes them; malformed or unavailable preference storage produces a bounded UI warning, while a valid selected workspace remains usable. The store may keep a bounded last-session identifier per workspace so app restart can resume through ACP. It contains no credentials, transcripts, Grok session files, or raw runtime diagnostics.

## Expanding access safely

Future issues may add native operations only when the workflow needs them. Each addition must:

1. expose a narrow application command rather than a broad plugin permission;
2. validate paths, URLs, and request scope in Rust;
3. return a bounded presentation DTO instead of native handles or raw diagnostics;
4. add only the exact capability permission needed by the `main` window and review application-command exposure before adding another window;
5. extend `tests/security-boundary.test.mjs` with the expected permission and explicit denials; and
6. document user-visible consequences and fail-closed behavior.

Filesystem, process, shell, dialog, HTTP, and opener plugins must not be added as shortcuts around this boundary. Grok credentials and session files remain outside the GUI's ownership.

## Verification

Run the boundary test, strict frontend build, Rust checks, and Tauri build from the repository root:

```powershell
npm test
npm run build
cargo check --workspace --all-targets
cargo test --workspace --all-targets
npm run tauri build -- --no-bundle
```

Launch the Windows development shell with:

```powershell
npm run tauri dev
```

The security test fails if a remote production URL appears, the capability differs from the exact event listen/unlisten allowlist, the asset protocol or dangerous CSP bypass is enabled, a broad native-access plugin is introduced, or the runtime bridge starts depending on ACP method names, raw payloads, transport IDs, or child-process commands.
