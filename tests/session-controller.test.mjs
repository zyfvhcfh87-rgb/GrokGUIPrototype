import assert from "node:assert/strict";
import test from "node:test";

import { createSessionController } from "../src/application/session-controller.ts";

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

const summary = (sessionId, workspace) => ({
  sessionId,
  workspace,
  title: "Fixture session",
  updatedAt: "2030-01-01T00:00:00Z",
});

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
      listSessions: async (request) => {
        calls.push({ command: "list", request });
        return (
          overrides.listSessions?.(request) ?? {
            sessions: [summary("session-001", request.workspace)],
            nextCursor: null,
            truncated: false,
          }
        );
      },
      newSession: async (request) => {
        calls.push({ command: "new", request });
        return overrides.newSession?.(request) ?? session;
      },
      loadSession: async (request) => {
        calls.push({ command: "load", request });
        return overrides.loadSession?.(request) ?? session;
      },
      resumeSession: async (request) => {
        calls.push({ command: "resume", request });
        return overrides.resumeSession?.(request) ?? session;
      },
      closeSession: async (request) => {
        calls.push({ command: "close", request });
        return overrides.closeSession?.(request) ?? { acknowledged: true };
      },
    },
  };
}

async function readyController(overrides = {}) {
  const harness = createBridge(overrides);
  const controller = createSessionController(harness.bridge);
  await controller.initialize();
  controller.setRuntime("ready", capabilities);
  return { controller, harness };
}

test("session controller lists through reviewed commands after a workspace is bound", async () => {
  const { controller, harness } = await readyController();

  await controller.setWorkspace({ path: "C:\\work" });

  assert.deepEqual(harness.calls, [
    { command: "list", request: { workspace: "C:\\work", cursor: null } },
  ]);
  assert.equal(controller.getState().listKind, "ready");
  assert.equal(controller.getState().sessions[0].sessionId, "session-001");
  assert.equal(controller.getState().sessions[0].status, "listed");
});

test("changing workspace clears the selected session before listing the next folder", async () => {
  const { controller, harness } = await readyController();
  await controller.setWorkspace({ path: "C:\\work" });
  await controller.openSession("session-001");
  assert.equal(controller.getState().selectedSessionId, "session-001");

  harness.calls.length = 0;
  await controller.setWorkspace({ path: "C:\\other" });

  assert.equal(controller.getState().selectedSessionId, null);
  assert.equal(controller.getState().selectedSession, null);
  assert.deepEqual(harness.calls, [
    { command: "list", request: { workspace: "C:\\other", cursor: null } },
  ]);
  assert.equal(
    controller.getState().sessions.every((item) => item.workspace === "C:\\other"),
    true,
  );
});

test("a session from another workspace cannot be opened against the selected folder", async () => {
  const { controller, harness } = await readyController({
    listSessions: () => ({
      sessions: [
        summary("session-001", "C:\\work"),
        summary("session-foreign", "C:\\other"),
      ],
      nextCursor: null,
      truncated: false,
    }),
  });
  await controller.setWorkspace({ path: "C:\\work" });
  harness.calls.length = 0;

  await assert.rejects(
    () => controller.openSession("session-foreign"),
    (error) => error.code === "invalid_workspace",
  );
  assert.equal(
    harness.calls.some((call) => call.command === "resume" || call.command === "load"),
    false,
  );
  assert.equal(
    controller.getState().sessions.some((item) => item.sessionId === "session-foreign"),
    false,
  );
});

test("create, resume, load, and close stay scoped to the bound workspace", async () => {
  const { controller, harness } = await readyController();
  await controller.setWorkspace({ path: "C:\\work" });
  harness.calls.length = 0;

  await controller.createSession();
  await controller.resumeSession("session-001");
  await controller.loadSession("session-001");
  await controller.closeSession("session-001");

  assert.deepEqual(
    harness.calls.filter((call) => call.command !== "list"),
    [
      { command: "new", request: { workspace: "C:\\work" } },
      { command: "resume", request: { sessionId: "session-001", workspace: "C:\\work" } },
      { command: "load", request: { sessionId: "session-001", workspace: "C:\\work" } },
      { command: "close", request: { sessionId: "session-001" } },
    ],
  );
  assert.equal(controller.getState().selectedSessionId, null);
  assert.equal(
    controller.getState().sessions.find((item) => item.sessionId === "session-001")?.status,
    "closed",
  );
});

test("empty, failed, selected, closed, and stale states are represented", async () => {
  const { controller, harness } = await readyController({
    listSessions: () => ({ sessions: [], nextCursor: null, truncated: false }),
  });
  await controller.setWorkspace({ path: "C:\\work" });
  assert.equal(controller.getState().listKind, "empty");

  const failing = await readyController({
    listSessions: async () => {
      throw {
        code: "protocol_request_failed",
        diagnostic: "list failed",
        recoverable: true,
      };
    },
  });
  await failing.controller.setWorkspace({ path: "C:\\work" });
  assert.equal(failing.controller.getState().listKind, "failed");

  const success = await readyController();
  await success.controller.setWorkspace({ path: "C:\\work" });
  await success.controller.openSession("session-001");
  assert.equal(success.controller.getState().sessions[0].status, "selected");
  await success.controller.closeSession("session-001");
  assert.equal(
    success.controller.getState().sessions.find((item) => item.sessionId === "session-001")
      ?.status,
    "closed",
  );

  success.harness.emit({
    generation: 1,
    sequence: 1,
    event: { type: "runtime_extension_invalidated", area: "sessions", sessionId: null },
  });
  await Promise.resolve();
  assert.equal(
    ["stale", "loading", "ready", "empty"].includes(success.controller.getState().listKind),
    true,
  );
});

test("opening prefers resume when the runtime advertises it", async () => {
  const { controller, harness } = await readyController();
  await controller.setWorkspace({ path: "C:\\work" });
  harness.calls.length = 0;

  await controller.openSession("session-001");

  assert.equal(harness.calls[0].command, "resume");
  assert.equal(controller.getState().selectedSessionId, "session-001");
});
