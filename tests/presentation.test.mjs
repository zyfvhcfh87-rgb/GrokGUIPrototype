import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  applyDocumentAppearance,
  clampPanelWidth,
  createPresentationController,
  DEFAULT_PRESENTATION_PREFERENCES,
  resolveTheme,
  sanitizePresentationPreferences,
} from "../src/application/presentation.ts";

test("panel widths stay in a conversation-preserving range", () => {
  assert.equal(clampPanelWidth(12), 200);
  assert.equal(clampPanelWidth(900), 420);
  assert.equal(clampPanelWidth(260), 260);
});

test("theme resolution honors explicit light and dark over the OS", () => {
  assert.equal(resolveTheme("light", true), "light");
  assert.equal(resolveTheme("dark", false), "dark");
  assert.equal(resolveTheme("system", false), "light");
});

test("onboarding skip and reopen persist without inventing other prefs", async () => {
  let stored = { ...DEFAULT_PRESENTATION_PREFERENCES, onboarding: "unseen" };
  const controller = createPresentationController({
    getPresentationPreferences: async () => stored,
    setPresentationPreferences: async (request) => {
      stored = request;
      return stored;
    },
  });
  await controller.initialize();
  await controller.setOnboarding("skipped");
  assert.equal(controller.getState().preferences.onboarding, "skipped");
  assert.equal(controller.getState().preferences.theme, "system");
  await controller.setOnboarding("unseen");
  assert.equal(controller.getState().preferences.onboarding, "unseen");
});

test("invalid stored presentation values fall back instead of crashing", () => {
  const sanitized = sanitizePresentationPreferences({
    theme: "neon",
    projectsWidth: -4,
    onboarding: "maybe",
  });
  assert.equal(sanitized.theme, "system");
  assert.equal(sanitized.projectsWidth, 200);
  assert.equal(sanitized.onboarding, "unseen");
});

test("applying appearance sets html data-theme for CSS without nesting", () => {
  const documentElement = { dataset: {}, style: {} };
  applyDocumentAppearance({
    theme: "light",
    reduceMotion: true,
    document: { documentElement },
  });
  assert.equal(documentElement.dataset.theme, "light");
  assert.equal(documentElement.dataset.motion, "reduce");
  assert.equal(documentElement.style.colorScheme, "light");
});

test("light theme CSS is flattened so body and chrome update without nesting", async () => {
  const css = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");
  assert.match(css, /html\[data-theme="light"\] body/u);
  assert.match(css, /html\[data-theme="light"\] \.titlebar/u);
  assert.doesNotMatch(css, /html\[data-theme="light"\] \{[\s\S]{0,80}body \{/u);
});