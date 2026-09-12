import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { matchShortcut, shortcutConsumesEvent, KEYBOARD_SHORTCUTS } from "../src/application/shortcuts.ts";

test("Windows-first shortcuts cover every documented cockpit action", () => {
  const ids = KEYBOARD_SHORTCUTS.map((item) => item.id);
  assert.deepEqual(new Set(ids).size, KEYBOARD_SHORTCUTS.length);
  assert.equal(matchShortcut({ key: "1", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false }), "toggle-projects");
  assert.equal(matchShortcut({ key: "2", ctrlKey: false, metaKey: true, altKey: false, shiftKey: false }), "toggle-details");
  assert.equal(matchShortcut({ key: "n", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false }), "new-session");
  assert.equal(matchShortcut({ key: "Enter", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false }), "send-prompt");
  assert.equal(matchShortcut({ key: "Escape", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false }), "cancel-or-dismiss");
  assert.equal(matchShortcut({ key: ",", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false }), "appearance");
  assert.equal(matchShortcut({ key: "/", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false }), "shortcuts");
  assert.equal(matchShortcut({ key: "o", ctrlKey: true, metaKey: false, altKey: false, shiftKey: true }), "onboarding");
  assert.equal(matchShortcut({ key: "r", ctrlKey: true, metaKey: false, altKey: false, shiftKey: true }), "compatibility");
  assert.equal(matchShortcut({ key: "1", ctrlKey: false, metaKey: false, altKey: true, shiftKey: false }), "focus-projects");
});

test("letter shortcuts do not steal typing in the composer except documented chords", () => {
  assert.equal(shortcutConsumesEvent("new-session", { tagName: "TEXTAREA" }), false);
  assert.equal(shortcutConsumesEvent("send-prompt", { tagName: "TEXTAREA" }), true);
  assert.equal(shortcutConsumesEvent("toggle-projects", { tagName: "TEXTAREA" }), true);
  assert.equal(shortcutConsumesEvent("new-session", { tagName: "BUTTON" }), true);
});