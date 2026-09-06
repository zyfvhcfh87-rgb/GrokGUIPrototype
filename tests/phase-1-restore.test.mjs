import assert from "node:assert/strict";
import test from "node:test";

import { createConversationController } from "../src/application/conversation-controller.ts";
import { createSessionController } from "../src/application/session-controller.ts";
import { createSetupController } from "../src/application/setup-controller.ts";

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
  sessionId: "session-001",
  models: null,
  legacyConfigOptions: [],
  controls: { modes: null, configOptions: [], truncated: false },
  truncated: false,
};

async function waitUntil(predicate) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (predicate()) {
      return;
    }
    await Promise.resolve();
  }
  throw new Error("condition was not met");
}

test("app restart restores workspace and session, then recovery ignores stale generation events", async () => {
  const calls = [];
  const listeners = new Set();
  const emit = (envelope) => {
    for (const listener of listeners) {
      listener(envelope);
    }
  };
  const recents = [
    {
      path: "C:\\work",
      available: true,
      lastSessionId: "session-001",
    },
  ];
  const onEvent = async (listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };

  const setup = createSetupController({
    onEvent,
    setupStatus: async () => ({
      runtimeAvailable: true,
      executableState: "available",
      executableSource: "user_install",
      failure: null,
    }),
    listRecentWorkspaces: async () => ({ workspaces: recents }),
    runtimeSnapshot: async () => ({
      generation: 0,
      lastSequence: 0,
      state: "disconnected",
      capabilities: null,
    }),
    startRuntime: async () => {
      calls.push("start");
      emit({
        generation: 1,
        sequence: 1,
        event: { type: "runtime_state_changed", state: "ready" },
      });
      return {
        generation: 1,
        lastSequence: 1,
        state: "ready",
        capabilities,
      };
    },
    restartRuntime: async () => {
      calls.push("restart");
      emit({
        generation: 2,
        sequence: 1,
        event: { type: "runtime_state_changed", state: "connecting" },
      });
      emit({
        generation: 2,
        sequence: 2,
        event: { type: "runtime_state_changed", state: "ready" },
      });
      return {
        generation: 2,
        lastSequence: 2,
        state: "ready",
        capabilities,
      };
    },
    validateWorkspace: async ({ path }) => ({ path }),
  });
  const sessions = createSessionController({
    onEvent,
    listSessions: async (request) => {
      calls.push({ command: "list", request });
      return {
        sessions: [
          {
            sessionId: "session-001",
            workspace: request.workspace,
            title: "Fixture session",
            updatedAt: "2030-01-01T00:00:00Z",
          },
        ],
        nextCursor: null,
        truncated: false,
      };
    },
    newSession: async () => session,
    loadSession: async (request) => {
      calls.push({ command: "load", request });
      return session;
    },
    resumeSession: async (request) => {
      calls.push({ command: "resume", request });
      return session;
    },
    closeSession: async () => ({ acknowledged: true }),
  });
  const conversation = createConversationController({
    onEvent,
    sendPrompt: async (request) => {
      calls.push({ command: "prompt", request });
      return { stopReason: "end_turn" };
    },
    cancelPrompt: async () => ({ acknowledged: true }),
    setSessionMode: async () => ({ acknowledged: true }),
    setSessionModel: async () => ({ acknowledged: true }),
    setSessionConfig: async () => ({ acknowledged: true }),
  });

  await sessions.initialize();
  await conversation.initialize();
  await setup.initialize();
  sessions.setRuntime(setup.getState().runtimeState, setup.getState().capabilities);
  conversation.setRuntime(setup.getState().runtimeState, setup.getState().capabilities);

  assert.equal(calls.includes("start"), true);
  assert.deepEqual(setup.getState().selectedWorkspace, { path: "C:\\work" });

  await sessions.setWorkspace(
    setup.getState().selectedWorkspace,
    setup.getState().recentWorkspaces[0].lastSessionId,
  );
  await waitUntil(() => sessions.getState().selectedSessionId === "session-001");
  conversation.setSession(sessions.getState().selectedSession);

  emit({
    generation: 1,
    sequence: 2,
    event: {
      type: "user_message_chunk_received",
      sessionId: "session-001",
      messageId: "u1",
      text: "hello",
      truncated: false,
    },
  });
  emit({
    generation: 1,
    sequence: 3,
    event: {
      type: "tool_call_changed",
      sessionId: "session-001",
      callId: "tool-1",
      title: "Inspect",
      kind: "tool",
      status: "completed",
      detail: "ok",
    },
  });
  emit({
    generation: 1,
    sequence: 4,
    event: {
      type: "message_chunk_received",
      sessionId: "session-001",
      messageId: "a1",
      text: "world",
      truncated: false,
    },
  });

  assert.deepEqual(
    conversation.presentation().cards.map((card) => card.type),
    ["user_message", "tool", "assistant_message"],
  );

  emit({
    generation: 1,
    sequence: 5,
    event: {
      type: "runtime_failed",
      diagnostic: "process lost",
      recoverable: true,
    },
  });
  sessions.setRuntime(setup.getState().runtimeState, setup.getState().capabilities);
  conversation.setRuntime(setup.getState().runtimeState, setup.getState().capabilities);
  assert.equal(conversation.presentation().canRecover, true);

  await setup.retry();
  sessions.setRuntime(setup.getState().runtimeState, setup.getState().capabilities);
  conversation.setRuntime(setup.getState().runtimeState, setup.getState().capabilities);
  conversation.setSession(sessions.getState().selectedSession);
  await waitUntil(() => sessions.getState().selectedSessionId === "session-001");
  conversation.setSession(sessions.getState().selectedSession);

  emit({
    generation: 1,
    sequence: 7,
    event: {
      type: "message_chunk_received",
      sessionId: "session-001",
      messageId: "stale",
      text: "stale generation",
      truncated: false,
    },
  });
  emit({
    generation: 2,
    sequence: 3,
    event: { type: "session_activated", sessionId: "session-001" },
  });
  emit({
    generation: 2,
    sequence: 4,
    event: {
      type: "message_chunk_received",
      sessionId: "session-001",
      messageId: "a2",
      text: "resumed",
      truncated: false,
    },
  });

  const texts = conversation
    .presentation()
    .cards.filter((card) => card.type === "assistant_message")
    .map((card) => card.text);
  assert.equal(texts.includes("stale generation"), false);
  assert.equal(texts.includes("resumed"), true);
  assert.equal(calls.includes("restart"), true);
  assert.equal(
    calls.filter((call) => call.command === "resume").length >= 2,
    true,
  );
});
