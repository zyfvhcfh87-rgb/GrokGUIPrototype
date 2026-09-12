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

test("queued presentation writes merge against the latest saved state", async () => {
  let stored = { ...DEFAULT_PRESENTATION_PREFERENCES };
  let releaseFirst;
  const firstGate = new Promise((resolve) => {
    releaseFirst = resolve;
  });
  let writes = 0;
  const controller = createPresentationController({
    getPresentationPreferences: async () => stored,
    setPresentationPreferences: async (request) => {
      writes += 1;
      if (writes === 1) {
        await firstGate;
      }
      stored = { ...request };
      return stored;
    },
  });
  await controller.initialize();
  const first = controller.update({ projectsOpen: false });
  const second = controller.update({ detailsOpen: false });
  releaseFirst();
  await Promise.all([first, second]);
  assert.equal(controller.getState().preferences.projectsOpen, false);
  assert.equal(controller.getState().preferences.detailsOpen, false);
  assert.equal(stored.projectsOpen, false);
  assert.equal(stored.detailsOpen, false);
});

test("queued toggles apply against the latest controller state", async () => {
  let stored = { ...DEFAULT_PRESENTATION_PREFERENCES };
  const controller = createPresentationController({
    getPresentationPreferences: async () => stored,
    setPresentationPreferences: async (request) => {
      stored = { ...request };
      return stored;
    },
  });
  await controller.initialize();
  await Promise.all([
    controller.update((current) => ({ projectsOpen: !current.projectsOpen })),
    controller.update((current) => ({ projectsOpen: !current.projectsOpen })),
  ]);
  assert.equal(controller.getState().preferences.projectsOpen, true);
});

test("app presentation writes are patches instead of full snapshots", async () => {
  const source = await readFile(new URL("../src/App.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(source, /presentation\.update\(\{ \.\.\.prefs/u);
  assert.match(source, /presentation\.update\(\(current\) => \(\{ projectsOpen: !current\.projectsOpen \}\)\)/u);
});

test("dialog focus trap is mounted once so live updates do not steal focus", async () => {
  const source = await readFile(new URL("../src/cockpit/Dialog.tsx", import.meta.url), "utf8");
  assert.match(source, /onCloseRef\.current = onClose/u);
  assert.doesNotMatch(source, /}, \[onClose\]\);/u);
});