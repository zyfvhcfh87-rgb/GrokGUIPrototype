# Tauri shell security boundary

Issue #6 establishes a deliberately small desktop boundary. The shell renders the packaged React application and grants the webview no filesystem, process, dialog, external-link, network, or general Tauri IPC permissions.

## Trust boundary

```text
packaged React assets
    -> one local Tauri webview (`main`)
    -> empty capability permission set
    -> no native access plugins or application commands
```

- Production content comes only from `../dist`; the main window URL is the local `index.html` entry point.
- Development uses the loopback-only Vite server at `127.0.0.1:1420`. It is the sole development CSP exception and is not present in the production CSP.
- Vite excludes `src-tauri` and the shared Cargo `target` directory from its watcher so Rust rebuilds cannot trigger frontend reloads or collide with locked Windows build artifacts.
- `withGlobalTauri` and the asset protocol are disabled.
- No remote origin is attached to a capability. The `main-shell` capability is local, targets only the `main` window, and starts with an empty permission list.
- Tauri's compile-time CSP rewriting remains enabled, and the production policy permits no remote script, style, font, frame, form, object, image, or network source.
- The Rust crate registers no application commands and initializes no plugins. The frontend therefore cannot reach the filesystem, spawn processes, show native dialogs, or open external URLs.

## Expanding access safely

Future issues may add native operations only when the workflow needs them. Each addition must:

1. expose a narrow application command rather than a broad plugin permission;
2. validate paths, URLs, and request scope in Rust;
3. return a bounded presentation DTO instead of native handles or raw diagnostics;
4. add only the exact capability permission needed by the `main` window;
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

The security test fails if a remote production URL appears, the capability gains a permission, the asset protocol or dangerous CSP bypass is enabled, or a broad native-access plugin is introduced.
