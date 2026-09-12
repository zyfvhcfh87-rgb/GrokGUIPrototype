import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { transformWithOxc } from "vite";
import test from "node:test";

let React;
let renderToStaticMarkup;
let OnboardingDialog;
let ShortcutsDialog;
let unavailable = false;

try {
  React = await import("react");
  ({ renderToStaticMarkup } = await import("react-dom/server"));
} catch (error) {
  if (error.code !== "ERR_MODULE_NOT_FOUND") {
    throw error;
  }
  unavailable = "React dependencies are unavailable; run npm ci before component tests.";
}

const transformCache = new Map();

async function transformModule(file) {
  const key = file.href;
  const cached = transformCache.get(key);
  if (cached !== undefined) {
    return cached;
  }
  const pending = (async () => {
    const source = await readFile(file, "utf8");
    let code = (await transformWithOxc(source, file.pathname, {
      jsx: { runtime: "automatic" },
    })).code;
    const specifiers = [...code.matchAll(/from "([^"]+)"/gu)].map((match) => match[1]);
    for (const specifier of specifiers) {
      if (specifier.startsWith("data:") || specifier.startsWith("file:")) {
        continue;
      }
      const rewritten = specifier.startsWith(".")
        ? new URL(specifier, file).pathname.endsWith(".tsx")
          ? await transformModule(new URL(specifier, file))
          : new URL(specifier, file).href
        : import.meta.resolve(specifier);
      code = code.replaceAll(`from "${specifier}"`, `from ${JSON.stringify(rewritten)}`);
    }
    return `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`;
  })();
  transformCache.set(key, pending);
  return pending;
}

if (!unavailable) {
  const onboardingHref = await transformModule(
    new URL("../src/cockpit/OnboardingDialog.tsx", import.meta.url),
  );
  const shortcutsHref = await transformModule(
    new URL("../src/cockpit/ShortcutsDialog.tsx", import.meta.url),
  );
  ({ OnboardingDialog } = await import(onboardingHref));
  ({ ShortcutsDialog } = await import(shortcutsHref));
}

test("onboarding dialog exposes skip, reopen copy, and a dialog label", { skip: unavailable }, () => {
  const html = renderToStaticMarkup(
    React.createElement(OnboardingDialog, {
      presentation: {
        steps: [
          { id: "welcome", title: "Welcome", detail: "Local cockpit", status: "current" },
          { id: "detect", title: "Find Grok Build", detail: "Looking", status: "upcoming" },
        ],
        current: "welcome",
        canContinue: true,
        canFinish: true,
        finishLabel: "Finish guide",
      },
      recovery: null,
      onStep: () => {},
      onSkip: () => {},
      onFinish: () => {},
      onPickWorkspace: () => {},
      onRetry: () => {},
    }),
  );
  assert.match(html, /role="dialog"/);
  assert.match(html, /Skip guide/);
  assert.match(html, /does not start, stop, or authenticate Grok/i);
});

test("shortcuts dialog lists keyboard-only operation", { skip: unavailable }, () => {
  const html = renderToStaticMarkup(React.createElement(ShortcutsDialog, { onClose: () => {} }));
  assert.match(html, /Ctrl\+1/);
  assert.match(html, /Ctrl\+N/);
  assert.match(html, /Escape/);
});
