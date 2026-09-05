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
