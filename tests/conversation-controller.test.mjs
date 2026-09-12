import assert from "node:assert/strict";
import test from "node:test";

import { createConversationController } from "../src/application/conversation-controller.ts";

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

const session = {
  sessionId: "session-1",
  models: {
    currentModelId: "fixture-fast",
    availableModels: [
      {
        modelId: "fixture-fast",
        name: "Fixture Fast",
        description: null,
        agentType: null,
        reasoningEffort: "low",
        reasoningEfforts: [
          { id: "low", label: "Low", description: null, value: "low", isDefault: true },
        ],
        supportsReasoningEffort: true,
        totalContextTokens: 1000,
        truncated: false,
      },
    ],
    truncated: false,
  },
  legacyConfigOptions: [],
  controls: {
    modes: {
      currentModeId: "default",
      availableModes: [{ id: "default", name: "Default", description: null }],
      truncated: false,
    },
    configOptions: [],
    truncated: false,
  },
  truncated: false,
};

function createBridge(overrides = {}) {
  const calls = [];
  let listener = null;
  return {
    calls,
    emit(event) {
      listener?.(event);
    },
    bridge: {
      onEvent: async (next) => {
        listener = next;
        return () => {
          listener = null;
        };
      },
      sendPrompt: async (request) => {
        calls.push({ command: "prompt", request });
        return overrides.sendPrompt?.(request) ?? { stopReason: "end_turn" };
      },
      cancelPrompt: async (request) => {
        calls.push({ command: "cancel", request });
        return overrides.cancelPrompt?.(request) ?? { acknowledged: true };
      },
      setSessionMode: async (request) => {
        calls.push({ command: "mode", request });
        return { acknowledged: true };
      },
      setSessionModel: async (request) => {
        calls.push({ command: "model", request });
        return { acknowledged: true };
      },
      setSessionConfig: async (request) => {
        calls.push({ command: "config", request });
        return { acknowledged: true };
      },
      openExternalUrl: async (request) => {
        calls.push({ command: "url", request });
        return overrides.openExternalUrl?.(request) ?? { acknowledged: true };
      },
    },
  };
}

async function readyController(overrides = {}) {
  const harness = createBridge(overrides);
  const controller = createConversationController(harness.bridge);
  await controller.initialize();
  controller.setRuntime("ready", capabilities);
  controller.setSession(session);
  return { controller, harness };
}

test("sendPrompt streams through the reviewed command and keeps an optimistic user card", async () => {
  const { controller, harness } = await readyController();
  controller.setDraft("hello");
  await controller.sendPrompt();

  assert.deepEqual(harness.calls, [
    { command: "prompt", request: { sessionId: "session-1", text: "hello" } },
  ]);
  assert.equal(controller.presentation().cards[0].type, "user_message");
  assert.equal(controller.presentation().cards[0].text, "hello");
});

test("prompting and cancellation disappear when the runtime does not advertise them", async () => {
  const { controller, harness } = await readyController();
  controller.setRuntime("ready", {
    ...capabilities,
    sessions: { ...capabilities.sessions, prompt: false, cancel: false },
  });
  controller.setDraft("hello");

  await assert.rejects(() => controller.sendPrompt(), (error) => error.code === "capability_unavailable");
  await assert.rejects(() => controller.cancelPrompt(), (error) => error.code === "capability_unavailable");
  assert.deepEqual(harness.calls, []);
});

test("model, mode, and config updates use advertised identifiers only", async () => {
  const { controller, harness } = await readyController();
  await controller.setModel("fixture-fast", "low");
  await controller.setMode("default");
  await controller.setConfig("verbose", { type: "boolean", value: true });

  assert.deepEqual(harness.calls, [
    {
      command: "model",
      request: { sessionId: "session-1", modelId: "fixture-fast", reasoningEffort: "low" },
    },
    { command: "mode", request: { sessionId: "session-1", modeId: "default" } },
    {
      command: "config",
      request: { sessionId: "session-1", configId: "verbose", value: { type: "boolean", value: true } },
    },
  ]);
});

test("in-place tool updates and later assistant chunks stay on the selected session", async () => {
  const { controller, harness } = await readyController();
  harness.emit({
    generation: 1,
    sequence: 1,
    event: { type: "session_activated", sessionId: "session-1" },
  });
  harness.emit({
    generation: 1,
    sequence: 2,
    event: {
      type: "tool_call_changed",
      sessionId: "session-1",
      callId: "tool-1",
      title: "Inspect",
      kind: "tool",
      status: "running",
      detail: null,
    },
  });
  harness.emit({
    generation: 1,
    sequence: 3,
    event: {
      type: "tool_call_changed",
      sessionId: "session-1",
      callId: "tool-1",
      title: "Inspect",
      kind: "tool",
      status: "completed",
      detail: "done",
    },
  });
  harness.emit({
    generation: 1,
    sequence: 4,
    event: {
      type: "message_chunk_received",
      sessionId: "session-other",
      messageId: "x",
      text: "wrong session",
      truncated: false,
    },
  });

  const cards = controller.presentation().cards;
  assert.equal(cards.length, 1);
  assert.equal(cards[0].status, "completed");
  assert.equal(cards[0].detail, "done");
});

test("failed prompts restore the draft and show an error card", async () => {
  const { controller } = await readyController({
    sendPrompt: async () => {
      throw {
        code: "protocol_request_failed",
        diagnostic: "turn failed",
        recoverable: true,
      };
    },
  });
  controller.setDraft("hello");
  await assert.rejects(() => controller.sendPrompt());
  assert.equal(controller.getState().draft, "hello");
  assert.equal(controller.presentation().cards.at(-1).type, "error");
});

test("external URLs are opened only through the reviewed command", async () => {
  const { controller, harness } = await readyController();
  await controller.openExternalUrl("https://example.com/docs");
  assert.deepEqual(harness.calls, [
    { command: "url", request: { url: "https://example.com/docs" } },
  ]);
});

async function workingController(overrides = {}) {
  const ready = await readyController(overrides);
  ready.harness.emit({
    generation: 1,
    sequence: 1,
    event: { type: "runtime_state_changed", state: "ready" },
  });
  ready.harness.emit({
    generation: 1,
    sequence: 2,
    event: { type: "session_activated", sessionId: "session-1" },
  });
  ready.harness.emit({
    generation: 1,
    sequence: 3,
    event: { type: "session_state_changed", sessionId: "session-1", state: "working" },
  });
  return ready;
}

test("cancel success stays cancelling until the turn settles as cancelled", async () => {
  const { controller, harness } = await workingController();
  const pending = controller.cancelPrompt();
  assert.equal(controller.presentation().composer.kind, "cancelling");
  assert.equal(controller.presentation().composer.canCancel, false);
  await pending;
  assert.equal(controller.getState().cancelling, true);
  harness.emit({
    generation: 1,
    sequence: 4,
    event: { type: "session_state_changed", sessionId: "session-1", state: "cancelled" },
  });
  assert.equal(controller.presentation().composer.kind, "cancelled");
  assert.deepEqual(harness.calls, [{ command: "cancel", request: { sessionId: "session-1" } }]);
});

test("duplicate cancel is idempotent and idle cancel does not target a turn", async () => {
  const { controller, harness } = await workingController();
  await controller.cancelPrompt();
  await controller.cancelPrompt();
  assert.deepEqual(harness.calls, [{ command: "cancel", request: { sessionId: "session-1" } }]);

  const idle = await readyController();
  assert.deepEqual(await idle.controller.cancelPrompt(), { acknowledged: true });
  assert.deepEqual(idle.harness.calls, []);
});

test("cancel rejection and timeout surface the failure without leaving a fake cancelled turn", async () => {
  const rejected = await workingController({
    cancelPrompt: async () => {
      throw {
        code: "protocol_request_failed",
        diagnostic: "runtime rejected cancellation",
        recoverable: true,
      };
    },
  });
  await assert.rejects(
    () => rejected.controller.cancelPrompt(),
    (error) => error.code === "protocol_request_failed",
  );
  assert.equal(rejected.controller.getState().cancelling, false);
  assert.equal(rejected.controller.presentation().composer.kind, "working");

  const timedOut = await workingController({
    cancelPrompt: async () => {
      throw {
        code: "request_timed_out",
        diagnostic: "runtime command timed out",
        recoverable: true,
      };
    },
  });
  await assert.rejects(
    () => timedOut.controller.cancelPrompt(),
    (error) => error.code === "request_timed_out",
  );
  assert.equal(timedOut.controller.getState().cancelling, false);
});

test("closing a session during sendPrompt clears sending before the prompt settles", async () => {
  const { promise, resolve } = Promise.withResolvers();
  const { controller, harness } = await readyController({
    sendPrompt: () => promise,
  });
  harness.emit({
    generation: 1,
    sequence: 1,
    event: { type: "session_activated", sessionId: "session-1" },
  });
  controller.setDraft("hello");
  const sending = controller.sendPrompt();
  assert.equal(controller.getState().sending, true);

  harness.emit({
    generation: 1,
    sequence: 2,
    event: { type: "session_state_changed", sessionId: "session-1", state: "closed" },
  });
  assert.equal(controller.getState().sending, false);

  resolve({ stopReason: "cancelled" });
  assert.deepEqual(await sending, { stopReason: "cancelled" });
  assert.equal(controller.getState().sending, false);
});

test("process loss during cancellation fails the conversation instead of restoring the turn", async () => {
  const { controller, harness } = await workingController();
  const pending = controller.cancelPrompt();
  harness.emit({
    generation: 1,
    sequence: 4,
    event: { type: "runtime_failed", diagnostic: "process lost", recoverable: true },
  });
  await pending;
  assert.equal(controller.presentation().kind, "failed");
  assert.equal(controller.getState().cancelling, false);
});

test("plan approval uses the advertised command and stays unavailable otherwise", async () => {
  const { controller, harness } = await readyController();
  harness.emit({
    generation: 1,
    sequence: 1,
    event: { type: "session_activated", sessionId: "session-1" },
  });
  harness.emit({
    generation: 1,
    sequence: 2,
    event: {
      type: "available_commands_changed",
      sessionId: "session-1",
      commands: [
        { name: "approve_plan", description: "Approve the current plan", acceptsInput: false },
        { name: "revise_plan", description: "Revise the current plan", acceptsInput: true },
      ],
      truncated: false,
    },
  });
  harness.emit({
    generation: 1,
    sequence: 3,
    event: {
      type: "plan_changed",
      sessionId: "session-1",
      entries: [{ id: "step-1", title: "Inspect", description: null, status: "pending" }],
      truncated: false,
    },
  });

  await controller.reviewPlan("approve");
  assert.deepEqual(harness.calls, [
    { command: "prompt", request: { sessionId: "session-1", text: "/approve_plan" } },
  ]);

  await controller.reviewPlan("revise");
  assert.equal(controller.getState().draft, "/revise_plan ");
  assert.equal(harness.calls.length, 1);

  const hidden = await readyController();
  await assert.rejects(
    () => hidden.controller.reviewPlan("approve"),
    (error) => error.code === "capability_unavailable",
  );
  assert.deepEqual(hidden.harness.calls, []);
});
