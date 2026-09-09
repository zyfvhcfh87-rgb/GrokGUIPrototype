import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { initialApplicationState, reduceApplicationEvent } from "../src/application/state.ts";

// Use the project's existing React and TypeScript dependencies; no test framework is added.
let React, renderToStaticMarkup, InteractionPane, ts;
let unavailable = false;
try {
  ts = await import("typescript");
  React = await import("react");
  ({ renderToStaticMarkup } = await import("react-dom/server"));

} catch (error) {
  if (error.code !== "ERR_MODULE_NOT_FOUND") throw error;
  unavailable = "React/TypeScript dependencies are unavailable; run npm ci before component tests.";
}

if (!unavailable) {
  const file = new URL("../src/cockpit/InteractionPane.tsx", import.meta.url);
  const source = await readFile(file, "utf8");
  const output = ts.transpileModule(source, { compilerOptions: {
    jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022,
  } }).outputText.replace(/from "([^"]+)"/gu, (_, specifier) =>
    `from ${JSON.stringify(specifier.startsWith(".") ? new URL(specifier, file).href : import.meta.resolve(specifier))}`);
  ({ InteractionPane } = await import(`data:text/javascript;base64,${Buffer.from(output).toString("base64")}`));
}

function render(request, override = {}) {
  const events = [
    { type: "runtime_state_changed", state: "ready" },
    { type: "session_activated", sessionId: "session-1" },
    { type: "session_state_changed", sessionId: "session-1", state: "working" }, request,
  ];
  const application = events.reduce((state, event, i) => reduceApplicationEvent(state, { generation: 1, sequence: i + 1, event }), initialApplicationState());
  const context = { application, sessionId: "session-1", epoch: 1, blocked: false, ...override };
  return renderToStaticMarkup(React.createElement(InteractionPane, { context, controller: {}, statuses: {} }));
}
const permission = {
  type: "permission_requested", sessionId: "session-1", interactionId: "p1", title: "Review command", consequence: "Overwrites the output file.",
  scope: { type: "command", command: "printf '<unsafe>' > out.txt", workingDirectory: "/workspace", affectedPaths: ["/workspace/out.txt", "/workspace/other.txt"] },
  availableDecisions: ["allow_once", "allow_always", "deny_once"],
};

test("permission component displays exact escaped scope and only advertised approval options", { skip: unavailable }, () => {
  const html = render(permission);
  for (const text of ["Working directory", "/workspace/out.txt", "/workspace/other.txt", "Overwrites the output file.", "Allow once", "Always allow", "Deny once"]) assert.ok(html.includes(text));
  assert.ok(html.includes("&lt;unsafe&gt;")); assert.ok(!html.includes("<unsafe>"));
  assert.ok(!html.includes("Always deny"));
  assert.ok(html.indexOf(">Deny once<") < html.indexOf(">Allow once<"));
});

test("malformed permission component exposes cancellation without an approval button", { skip: unavailable }, () => {
  const html = render({ ...permission, scope: { type: "other" } });
  assert.ok(html.includes("Approval is unavailable"));
  assert.ok(html.includes("Cancel request")); assert.ok(!html.includes(">Allow once<"));
});

test("sensitive text component is labelled, bounded, masked, and has no implicit submit", { skip: unavailable }, () => {
  const html = render({ type: "elicitation_requested", sessionId: "session-1", interactionId: "e1", prompt: "Enter a value", control: {
    type: "text", fieldId: "answer", label: "Your value", sensitive: true, placeholder: "do-not-display-default", minLength: 1, maxLength: 64,
  } });
  assert.match(html, /type="password"/); assert.match(html, /autoComplete="off"/i);
  assert.match(html, /maxLength="128"/i); assert.match(html, /<label for="/);
  assert.ok(!html.includes("do-not-display-default")); assert.ok(!html.includes('type="submit"'));
  assert.ok(html.indexOf(">Cancel request<") < html.indexOf(">Accept<"));
});

test("confirmation and choice components use native keyboard accessible controls", { skip: unavailable }, () => {
  const request = control => ({ type: "elicitation_requested", sessionId: "session-1", interactionId: "e1", prompt: "Choose", control });
  const boolean = render(request({ type: "confirmation", fieldId: "confirm", label: "Continue?" }));
  assert.equal((boolean.match(/type="radio"/g) ?? []).length, 2); assert.ok(!boolean.includes(' checked=""'));
  const choice = render(request({ type: "choice", fieldId: "color", label: "Color", options: ["red", "blue"], multiple: false, truncated: false }));
  assert.match(choice, /<select/); assert.ok(choice.includes("Choose an option"));
  const multiple = render(request({ type: "choice", fieldId: "color", label: "Colors", options: ["red", "blue"], multiple: true, truncated: false }));
  assert.equal((multiple.match(/type="checkbox"/g) ?? []).length, 2);
});

test("unsupported and truncated forms offer cancellation only", { skip: unavailable }, () => {
  for (const control of [{ type: "other" }, { type: "choice", fieldId: "color", label: null, options: ["red"], multiple: false, truncated: true }]) {
    const html = render({ type: "elicitation_requested", sessionId: null, interactionId: "e1", prompt: "Choose", control });
    assert.ok(html.includes("Cancel request")); assert.ok(!html.includes(">Accept<"));
  }
});

test("blocked, orphaned and expired requests do not render actionable cards", { skip: unavailable }, () => {
  assert.equal(render(permission, { blocked: true }), "");
  assert.equal(render({ ...permission, sessionId: "orphan" }), "");
  assert.equal(render(permission, { application: initialApplicationState(2) }), "");
});
