# Issue 13: permission and elicitation surfaces

Implementation is prepared against the source tree `b98280c30615702423c4d0897497bddc0a595b70` from `main`, inspected on 2026-09-09. This is not a claim of live Grok compatibility or completion of desktop QA.

## Behavior

- Permission cards show exact command text, working directory, every supplied affected path, and the supplied consequence. Only advertised approval and denial decisions are offered; cancellation remains available as a supported application intent. Persistent decisions explicitly say that Grok controls their matching rules and duration.
- Missing, relative, unsupported, or ambiguous scopes cannot approve. The runtime rejects multi-path filesystem operations and moves that the current single-path scope cannot represent completely. Command scopes retain all valid supplied locations.
- Text, confirmation, single-choice and multiple-choice controls use native labelled inputs. No answer or permission is accepted implicitly by Enter in a text input. Escape cancels forms. Cancel/deny precedes approval in keyboard order.
- Text limits use Unicode character counts plus a 16 KB UTF-8 ceiling, checked in both the response controller and Rust. Free text is masked conservatively; runtime defaults are not exposed. Form values live only in the component and are cleared on submit, cancel, and unmount. Response errors use static text, never transport error contents.
- Response callbacks recheck the current generation, session selection, request identity, pending event gaps, and cancellation/navigation state. A submission is claimed synchronously. Its lock survives temporary UI blocking and navigation, and uncertain delivery cannot be retried as an approval.
- The reducer rejects orphaned interaction events and keeps bounded tombstones so duplicate or resolved IDs cannot resurrect cards. The native interaction gate also rejects unknown sessions and serializes responses against close/cancel/shutdown.

## Supported runtime subset

The existing ACP adapter accepts one-field string/boolean/enum forms. It now preserves string length bounds and rejects unsupported string formats, conflicting bounds, duplicate choices and unknown required fields. Multi-field, numeric, URL and other unsupported forms remain fail-closed. Multiple-choice presentation supports the existing normalized DTO, but the current standard ACP adapter does not yet emit array controls.

Live installed-runtime permission and elicitation behavior remains unconfirmed, as recorded in the Phase 0 evidence. No Grok binary was available in this environment and no paid model calls were made.

## Verification on 2026-09-09

`node --test tests/*.test.mjs`: 79 passed, 0 failed, 6 component tests skipped because the declared React/TypeScript dependencies are unavailable. Existing contract-manifest, reducer, controller, and security tests pass. New tests cover every permission decision, malformed scopes/options, typed form validation, sensitive input handling, cancellation, closed/failed/completed sessions, restart, orphaned requests, duplicate delivery, resolved IDs, event gaps, navigation, and uncertain delivery.

Six React rendering tests are included for exact escaped scope, supported actions, masked/labelled/bounded input, native choice controls, unsupported forms, and expired requests. They require the project's existing dependencies and add no package dependencies. New Rust tests cover path preservation, ambiguous option IDs, text limits/default secrecy, orphan/session gates, and close-before-approval delivery; they were not run because Rust/Cargo is unavailable.

Original outstanding verification commands:

```sh
npm ci
npm run check
npm test
npm run build
cargo fmt --all --check
cargo test -p grok-runtime
cargo test -p grok-build-gui
```

Run the Windows fake-process contract suite and packaged desktop flows in the supported environment.

For manual frontend QA, run `npm run dev` and open `http://127.0.0.1:1420/?fixture=interactions`. The fixture exposes a permission card, masked text form, and choice form without invoking a real runtime or executing the displayed command. Check Tab/Shift+Tab navigation, Enter/Escape behavior, denial, persistent approval, invalid/valid input, cancel, and session close. The fixture is development-only and is not included as a production access path.

Native build, React render tests, browser keyboard interaction, screenshots, and packaged desktop validation remain outstanding. Do not close issue 13 based on the unit-test result alone.


## Follow-up QA on macOS, 2026-09-09

**Status: local automated and interaction-fixture QA passed; issue 13 is not yet qualified for closure.** This follow-up starts from merged PR #27, commit `4da15c21b91fcad5f54fd11ef17ed377c71a4c43`, with the local QA fixes described below. The subsequent Windows CI results are tracked in [PR #28](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/pull/28); the table below records the local run.

Environment: Apple Silicon macOS, Node 26.8.1, TypeScript 7.0.2, Vite 8.2.2, installed Grok 1.0.25 (`f7e67d6988e2`, stable). No credential contents were read and no model prompts were submitted to the installed runtime.

### Defects found and repaired

- The component harness imported `typescript.transpileModule` / `JsxEmit`, which the declared TypeScript 7 package does not expose. It now uses the existing Vite Oxc transformer. All six rendering tests execute; none are skipped.
- The Windows-gated runtime-service test reused a moved workspace `PathBuf`, preventing compilation when enabled. Clone the path before passing it to the list request.
- Enable the platform-independent runtime-service suite on macOS as well as Windows. The fake permission request now uses absolute paths appropriate to the host; Windows retains the existing fixture paths. This exercises the actual SDK process transport and interaction responses on macOS without weakening production scope validation.
- Increase the hanging-close fixture's shared request timeout from 100 ms to two seconds. The 100 ms limit expired during initialization under parallel process startup, before the intended close assertion. The fixture still never answers close, and the test still verifies timeout, fail-closed behavior, and explicit reactivation.
- Apply `cargo fmt` across the workspace; production Rust changes are formatting only.

### Results

| Check | Result |
| --- | --- |
| `npm ci` | Passed; audit reported zero vulnerabilities |
| `npm run check` | Passed |
| `npm test` | 85 passed, zero failed, zero skipped |
| `npm run build` | Passed |
| `cargo fmt --all --check` | Passed after formatting |
| `cargo test --workspace` | 183 passed, zero failed; Windows-only targets excluded by cfg |
| Runtime-service process tests, included above | 13 passed, including permission/elicitation acceptance, cancellation, close, ambiguous close, stale IDs, crash/restart, and process cleanup |
| `git diff --check` | Passed |
| `npm run tauri build -- --debug --bundles app` | macOS development app bundle produced successfully |
| Packaged app launch | Local assets loaded; installed Grok reached Ready, ACP v1, session list/new/load/resume/close advertised, authentication handled by Grok |
| Packaged full interaction workflow | Blocked: selecting a workspace reports `native workspace selection is unavailable on this platform` |
| Windows transport, tests and packaging | Not run locally; `.github/workflows/windows-qa.yml` runs on PR #28, with final results linked there |

The Rust build also emits non-fatal macOS warnings about unused Windows diagnostic helpers and the unavailable native picker branch. No warning-free build claim is made.

### Native WebKit interaction QA

Run this checkout's Vite server on a verified free port (`1423` was used; `1420` belonged to another checkout). A separate temporary Tauri development app loaded `http://127.0.0.1:1423/?fixture=interactions`. This is a development fixture through the application bridge, **not** a packaged production interaction or live Grok approval test.

Confirmed by native UI actions and accessibility/screenshot inspection:

- Exact command, working directory, affected file, consequence, and runtime-controlled persistence explanation are visible.
- Deny once removes the permission card; Always allow resolves its own card and leaves unrelated forms pending. These fixture actions do not execute the displayed command or persist a real permission.
- Empty text and an unselected choice show validation errors.
- Entered test text is masked; Enter in the field does not approve it.
- Tab from the text field focuses Cancel before Accept, with a visible focus ring. Shift+Tab returns to the field; Escape cancels and removes the form.
- Keyboard selection of Markdown followed by explicit keyboard activation of Accept resolves the choice form.
- A valid masked label is accepted using Tab/Tab/Enter on the explicit Accept button.
- Turn cancellation expires the remaining form and returns the composer to Ready.

### Remaining closure gate

1. Run the prepared Windows QA workflow on the QA commit. Require the frontend, Rust workspace (including Windows-only transport/process targets), formatting, and NSIS development packaging steps to pass.
2. In the Windows packaged application, verify permission allow/deny, elicitation accept/cancel, turn cancellation, close, crash/restart, and stale-request expiry through the native command/event bridge. Record the tested commit, runtime version/capabilities, and sanitized results. Development fixture checks above do not replace this step.
3. Publish the QA fixes and evidence, then close #13 only once the remaining desktop qualification is satisfied. The macOS picker limitation is outside this permission/elicitation change and must not be represented as a completed desktop workflow.


### Windows CI follow-up

The first Windows run compiled successfully and exposed four outdated assertions in `managed_restart_faults.rs`: they expected only `created\n`, while the workspace-aware fixture intentionally persists that marker plus its synthetic workspace. The assertions now check the exact canonical workspace record and continue to forbid session identifiers or prompt content. The close marker remains fixed and payload-free. The workflow uses `--no-fail-fast` so failures in one Rust target do not hide results from subsequent targets.

[PR #28](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/pull/28) records the final tested commit, CI test totals and installer artifact. Successful automated CI does not by itself complete the packaged Windows interaction closure gate above.
