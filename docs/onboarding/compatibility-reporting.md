# Onboarding and privacy-safe compatibility reporting

**Issue:** [#18](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/18)

First launch opens a setup guide: welcome → find Grok → advertised sign-in → workspace → compatibility. Skip and Finish only write GUI presentation preferences. They do not start, stop, or authenticate Grok.

Reopen from **Setup guide** or Ctrl+Shift+O. That does not reset runtime state.

## Compatibility report

**Compatibility** (Ctrl+Shift+R) copies JSON built from `setup_status` plus the negotiated snapshot:

- App name/version
- Runtime state, executable state/source (never the path)
- Advertised agent product/version and ACP protocol version
- Advertised auth method IDs
- Session capability flags
- Model catalog count/truncated, not session IDs or prompt text

Excluded: credentials, private paths, session IDs, prompt text, stderr, workspace contents.

Clipboard copy uses the webview clipboard. If that is unavailable, select the JSON field and copy manually.
