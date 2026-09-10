import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { projectConversation } from "../src/application/conversation.ts";
import { initialApplicationState, reduceApplicationEvent } from "../src/application/state.ts";

let React;
let renderToStaticMarkup;
let ActivityPane;
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

if (!unavailable) {
  try {
    const { transformWithOxc } = await import("vite");
    const file = new URL("../src/cockpit/ActivityPane.tsx", import.meta.url);
    const source = await readFile(file, "utf8");
    const output = (
      await transformWithOxc(source, file.pathname, {
        jsx: { runtime: "automatic" },
      })
    ).code.replace(/from "([^"]+)"/gu, (_, specifier) =>
      `from ${JSON.stringify(specifier.startsWith(".") ? new URL(specifier, file).href : import.meta.resolve(specifier))}`,
    );
    ({ ActivityPane } = await import(
      `data:text/javascript;base64,${Buffer.from(output).toString("base64")}`
    ));
  } catch (error) {
    if (error.code !== "ERR_MODULE_NOT_FOUND") {
      throw error;
    }
    unavailable = "Vite is unavailable; run npm ci before component tests.";
  }
}

const capabilities = {
  protocolVersion: 1,
  agent: { product: "grok_build", version: "1.0.5" },
  authenticationMethods: ["cached_token"],
  sessions: {
    create: true,
    prompt: true,
    cancel: true,
    list: true,
    load: true,
    resume: true,
    close: true,
  },
  models: null,
  truncated: false,
};

function presentation(commands) {
  const events = [
    { type: "session_activated", sessionId: "session-1" },
    { type: "session_state_changed", sessionId: "session-1", state: "completed" },
    {
      type: "available_commands_changed",
      sessionId: "session-1",
      commands,
      truncated: false,
    },
    {
      type: "plan_changed",
      sessionId: "session-1",
      entries: [{ id: "step-1", title: "Inspect workspace", description: null, status: "pending" }],
      truncated: false,
    },
  ];
  const application = events.reduce(
    (state, event, index) =>
      reduceApplicationEvent(state, { generation: 1, sequence: index + 1, event }),
    initialApplicationState(),
  );
  return projectConversation({
    sessionId: "session-1",
    session: null,
    sessionView: application.sessions["session-1"],
    capabilities,
    runtimeState: "ready",
    modelCatalog: null,
    currentModeId: null,
    configOptions: [],
    pendingUserMessages: [],
    sending: false,
    cancelling: false,
    failure: null,
    needsResync: false,
    runtimeFailure: null,
  });
}

function render(commands) {
  const clicks = [];
  const html = renderToStaticMarkup(
    React.createElement(ActivityPane, {
      presentation: presentation(commands),
      controlBusy: false,
      onModelChange() {},
      onReasoningChange() {},
      onModeChange() {},
      onConfigChange() {},
      onInsertCommand() {},
      onApprovePlan() {
        clicks.push("approve");
      },
      onRevisePlan() {
        clicks.push("revise");
      },
    }),
  );
  return { html, clicks };
}

test("activity pane shows replaced plan actions only when advertised", { skip: unavailable }, () => {
  const hidden = render([]);
  assert.ok(hidden.html.includes("Saved plan"));
  assert.ok(hidden.html.includes("Inspect workspace"));
  assert.ok(!hidden.html.includes(">Approve<"));
  assert.ok(!hidden.html.includes(">Revise<"));

  const shown = render([
    { name: "approve_plan", description: "Approve the current plan", acceptsInput: false },
    { name: "revise_plan", description: "Revise the current plan", acceptsInput: true },
  ]);
  assert.ok(shown.html.includes(">Approve<"));
  assert.ok(shown.html.includes(">Revise<"));
});
