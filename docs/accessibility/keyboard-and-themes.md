# Keyboard, themes, and accessibility

**Issue:** [#17](https://github.com/zyfvhcfh87-rgb/GrokGUIPrototype/issues/17)

The three-region cockpit is keyboard-first. Side panels collapse and resize without shrinking the conversation below 390px. Appearance is GUI-owned (`presentation.json`); GrokRuntime is unchanged.

## Shortcuts (Windows-first)

Ctrl chords also accept Command.

| Shortcut | Action |
| --- | --- |
| Ctrl+1 | Show or hide projects |
| Ctrl+2 | Show or hide details |
| Alt+1 | Focus projects |
| Alt+2 | Move focus to conversation |
| Ctrl+L | Focus the prompt |
| Ctrl+N | New session |
| Ctrl+Enter | Send prompt (Enter still sends from the composer) |
| Escape | Close a dialog, or cancel the active turn |
| Ctrl+, | Appearance |
| Ctrl+/ | Shortcut list |
| Ctrl+Shift+O | Reopen setup guide |
| Ctrl+Shift+R | Compatibility report |

Session list: Arrow keys, Home/End, Delete/Backspace to close. Skip link: Skip to conversation.

## Themes and motion

Theme: system, light, or dark. Motion: system or reduce. High contrast follows the OS. Reduced motion disables pulse and shortens transitions.

## Manual keyboard and screen-reader pass

Use `npm run tauri dev` or `npm run dev` with `?fixture=conversation`, `?fixture=interactions`, and `?fixture=onboarding`.

1. Tab from the skip link through titlebar, panels, composer, and dialogs. Focus returns to the opener after Close/Escape.
2. Collapse projects while a session button is focused; focus moves to Projects. Conversation stays wide.
3. Resize panels with the separator (Arrow keys and pointer).
4. Pending permission: "Destructive action" or "Review required" is visible as text, not color alone. Deny precedes Allow.
5. Screen reader: runtime status live region, dialog labels, session listbox.
6. Light theme, reduced motion, OS high contrast, and a ~720px-wide window.

This host recorded automated checks. A packaged Windows Narrator pass remains a Lyn checklist item on the NSIS artifact.
