# Phase 3 polish evidence

**Issues:** [#17](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/17), [#18](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/18), [#19](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/19)  
**Recorded:** 2026-09-12  
**Host:** Linux cloud agent, no Grok binary

| Area | Classification | Notes |
| --- | --- | --- |
| Keyboard map, collapse, resize, themes, live status, destructive permission text | Confirmed (automated + fixture HTML + browser) | Light theme uses flattened `html[data-theme="light"]` selectors so body/chrome update. Manual pass on `?fixture=conversation`, `interactions`, `onboarding` |

| Onboarding skip/reopen | Confirmed (unit + fixture) | Presentation store + onboarding presentation. `?fixture=onboarding` |
| Compatibility report privacy | Confirmed | `tests/compatibility.test.mjs` |
| Recovery kinds | Confirmed | `tests/onboarding.test.mjs` |
| Packaging config: NSIS, no updater, no Grok, no signing secret | Confirmed | `tests/packaging.test.mjs`, `tauri.conf.json` |
| Exit stops contained runtime | Confirmed (code + security-boundary) | `stop_contained_runtime` on `RunEvent::Exit` / `ExitRequested` |
| Packaged Windows install/launch | Explicitly unknown on this host | `windows-qa.yml` builds NSIS on `windows-latest` |
| Live packaged Grok prompt/permission/crash/resume | Explicitly unknown | Same as Phase 2; needs Windows + installed Grok |
| Narrator pass on NSIS | Explicitly unknown | Manual checklist in `docs/accessibility/keyboard-and-themes.md` |

GrokRuntime public surface is unchanged. Presentation commands are GUI-owned.
