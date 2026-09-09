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

Before merging, run:

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
