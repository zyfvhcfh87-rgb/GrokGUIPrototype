import assert from "node:assert/strict";
import test from "node:test";

import {
  MAX_TERMINAL_DISPLAY_CHARS,
  describeComposer,
  describeSessionControls,
  projectConversation,
  projectConversationCards,
} from "../src/application/conversation.ts";
import { initialApplicationState, reduceApplicationEvent } from "../src/application/state.ts";

function initialSessionForTest() {
  return {
    state: "ready",
    title: null,
    updatedAt: null,
    timeline: [],
    messages: {},
    thoughts: {},
    toolCalls: {},
    permissions: {},
    elicitations: {},
    plan: [],
    usage: null,
    availableCommands: [],
    currentModeId: null,
    configOptions: [],
    metadataRevision: 0,
    invalidatedAreas: [],
  };
}

function reduceThrough(events) {
  return events.reduce(
    (state, event, index) =>
      reduceApplicationEvent(state, { generation: 1, sequence: index + 1, event }),
    initialApplicationState(),
  );
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

test("streamed events become structured cards in timeline order", () => {
  const sessionView = reduceThrough([
    { type: "session_activated", sessionId: "session-1" },
    {
      type: "user_message_chunk_received",
      sessionId: "session-1",
      messageId: "u1",
      text: "hello",
      truncated: false,
    },
    {
      type: "thought_chunk_received",
      sessionId: "session-1",
      thoughtId: "t1",
      text: "thinking",
      truncated: false,
    },
    {
      type: "tool_call_changed",
      sessionId: "session-1",
      callId: "term-1",
      title: "List",
      kind: "terminal_command",
      status: "completed",
      detail: "ok",
    },
    {
      type: "message_chunk_received",
      sessionId: "session-1",
      messageId: "a1",
      text: "hi",
      truncated: false,
    },
    {
      type: "tool_call_changed",
      sessionId: "session-1",
      callId: "bg-1",
      title: "Watch",
      kind: "background_task",
      status: "running",
      detail: null,
    },
  ]).sessions["session-1"];

  const cards = projectConversationCards({
    sessionView,
    pendingUserMessages: [],
    failure: null,
    runtimeFailure: null,
  });

  assert.deepEqual(
    cards.map((card) => card.type),
    ["user_message", "thought", "tool", "assistant_message", "tool"],
  );
  assert.equal(cards[2].untrusted, true);
  assert.equal(cards[2].kindLabel, "Terminal");
  assert.equal(cards[4].kindLabel, "Background");
  assert.equal(cards[4].statusLabel, "Running");
});

test("terminal output is bounded and treated as untrusted display text", () => {
  const detail = "x".repeat(MAX_TERMINAL_DISPLAY_CHARS + 25);
  const cards = projectConversationCards({
    sessionView: {
      ...initialSessionForTest(),
      timeline: [{ kind: "tool", id: "term-1" }],
      toolCalls: {
        "term-1": {
          id: "term-1",
          title: "Dump",
          kind: "terminal_command",
          status: "completed",
          detail,
        },
      },
    },
    pendingUserMessages: [],
    failure: null,
    runtimeFailure: null,
  });

  assert.equal(cards[0].detail.length, MAX_TERMINAL_DISPLAY_CHARS);
  assert.equal(cards[0].detailTruncated, true);
  assert.equal(cards[0].untrusted, true);
});

test("error states from prompt, session, and runtime become cards", () => {
  const cards = projectConversationCards({
    sessionView: { ...initialSessionForTest(), state: "failed" },
    pendingUserMessages: [],
    failure: {
      code: "protocol_request_failed",
      diagnostic: "prompt failed",
      recoverable: true,
    },
    runtimeFailure: { diagnostic: "process lost", recoverable: true },
  });

  assert.deepEqual(
    cards.filter((card) => card.type === "error").map((card) => card.title),
    ["Request failed", "Session failed", "Runtime failed"],
  );
});

test("controls render only advertised models, modes, commands, and options", () => {
  const hidden = describeSessionControls({
    session: {
      sessionId: "session-1",
      models: null,
      legacyConfigOptions: [],
      controls: { modes: null, configOptions: [], truncated: false },
      truncated: false,
    },
    sessionView: initialSessionForTest(),
    capabilities,
    modelCatalog: null,
    currentModeId: null,
    configOptions: [],
  });
  assert.equal(hidden.canChangeModel, false);
  assert.equal(hidden.canChangeReasoning, false);
  assert.equal(hidden.canChangeMode, false);
  assert.equal(hidden.canChangeConfig, false);
  assert.deepEqual(hidden.models, []);
  assert.deepEqual(hidden.modes, []);
  assert.deepEqual(hidden.commands, []);

  const shown = describeSessionControls({
    session: {
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
        configOptions: [
          {
            id: "verbose",
            name: "Verbose",
            description: null,
            kind: { type: "boolean", currentValue: false },
          },
        ],
        truncated: false,
      },
      truncated: false,
    },
    sessionView: {
      ...initialSessionForTest(),
      availableCommands: [{ name: "review", description: "Review", acceptsInput: false }],
    },
    capabilities,
    modelCatalog: null,
    currentModeId: "default",
    configOptions: [],
  });

  assert.deepEqual(
    shown.models.map((model) => model.id),
    ["fixture-fast"],
  );
  assert.deepEqual(
    shown.modes.map((mode) => mode.id),
    ["default"],
  );
  assert.deepEqual(
    shown.commands.map((command) => command.name),
    ["review"],
  );
  assert.equal(shown.canChangeReasoning, true);
  assert.equal(shown.canChangeConfig, true);
});

test("composer working, waiting, and cancel states follow advertised support", () => {
  const working = describeComposer({
    sessionId: "session-1",
    sessionState: "working",
    capabilities,
    runtimeState: "working",
    sending: false,
    cancelling: false,
  });
  assert.equal(working.kind, "working");
  assert.equal(working.canSend, false);
  assert.equal(working.canCancel, true);

  const waiting = describeComposer({
    sessionId: "session-1",
    sessionState: "waiting_for_input",
    capabilities,
    runtimeState: "waiting_for_input",
    sending: false,
    cancelling: false,
  });
  assert.equal(waiting.kind, "waiting");
  assert.equal(waiting.canSend, true);

  const noCancel = describeComposer({
    sessionId: "session-1",
    sessionState: "working",
    capabilities: {
      ...capabilities,
      sessions: { ...capabilities.sessions, cancel: false },
    },
    runtimeState: "working",
    sending: false,
    cancelling: false,
  });
  assert.equal(noCancel.canCancel, false);

  const unavailable = describeComposer({
    sessionId: "session-1",
    sessionState: "ready",
    capabilities: {
      ...capabilities,
      sessions: { ...capabilities.sessions, prompt: false },
    },
    runtimeState: "ready",
    sending: false,
    cancelling: false,
  });
  assert.equal(unavailable.kind, "unavailable");
  assert.equal(unavailable.canSend, false);
});

test("the conversation surface stays presentable when a session is selected", () => {
  const presentation = projectConversation({
    sessionId: "session-1",
    session: null,
    sessionView: initialSessionForTest(),
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
  assert.equal(presentation.kind, "empty");
  assert.equal(presentation.composer.kind, "ready");
  assert.equal(presentation.canRecover, false);
});

test("failed and disconnected conversations expose a recoverable action", () => {
  const failed = projectConversation({
    sessionId: "session-1",
    session: null,
    sessionView: initialSessionForTest(),
    capabilities,
    runtimeState: "failed",
    modelCatalog: null,
    currentModeId: null,
    configOptions: [],
    pendingUserMessages: [],
    sending: false,
    cancelling: false,
    failure: null,
    needsResync: false,
    runtimeFailure: { diagnostic: "process lost", recoverable: true },
  });
  assert.equal(failed.kind, "failed");
  assert.equal(failed.canRecover, true);

  const disconnected = projectConversation({
    sessionId: "session-1",
    session: null,
    sessionView: initialSessionForTest(),
    capabilities,
    runtimeState: "disconnected",
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
  assert.equal(disconnected.kind, "failed");
  assert.equal(disconnected.canRecover, true);

  const incompatible = projectConversation({
    sessionId: "session-1",
    session: null,
    sessionView: initialSessionForTest(),
    capabilities,
    runtimeState: "failed",
    modelCatalog: null,
    currentModeId: null,
    configOptions: [],
    pendingUserMessages: [],
    sending: false,
    cancelling: false,
    failure: {
      code: "unsupported_protocol",
      diagnostic: "ACP protocol v1 was not negotiated",
      recoverable: false,
    },
    needsResync: false,
    runtimeFailure: { diagnostic: "incompatible", recoverable: false },
  });
  assert.equal(incompatible.canRecover, false);
});
