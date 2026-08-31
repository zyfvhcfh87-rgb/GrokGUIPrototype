import assert from "node:assert/strict";
import test from "node:test";

import { createSetupController } from "../src/application/setup-controller.ts";

const setup = {
  runtimeAvailable: true,
  executableState: "available",
  executableSource: "user_install",
  failure: null,
};

const snapshot = {
  generation: 0,
  lastSequence: 0,
  state: "disconnected",
  capabilities: null,
};

test("setup controller subscribes before it inspects and starts the runtime", async () => {
  const calls = [];
  let listener = null;
  const controller = createSetupController({
    onEvent: async (next) => {
      calls.push("listen");
      listener = next;
      return () => calls.push("unlisten");
    },
    setupStatus: async () => {
      calls.push("setup");
      return setup;
    },
    listRecentWorkspaces: async () => {
      calls.push("recent");
      return { workspaces: [] };
    },
    runtimeSnapshot: async () => {
      calls.push("snapshot");
      return snapshot;
    },
    startRuntime: async () => {
      calls.push("start");
      listener({
        generation: 1,
        sequence: 1,
        event: { type: "runtime_state_changed", state: "authenticating" },
      });
      listener({
        generation: 1,
        sequence: 2,
        event: { type: "runtime_state_changed", state: "ready" },
      });
      return { ...snapshot, generation: 1, lastSequence: 2, state: "ready" };
    },
  });

  await controller.initialize();

  assert.equal(calls[0], "listen");
  assert.equal(calls.at(-1), "start");
  assert.equal(controller.getState().runtimeState, "ready");
  assert.equal(controller.getState().setup, setup);
});

test("setup controller opens and removes recent workspaces through reviewed commands", async () => {
  const controller = createSetupController({
    onEvent: async () => () => {},
    setupStatus: async () => setup,
    listRecentWorkspaces: async () => ({
      workspaces: [{ path: "C:\\old", available: false }],
    }),
    runtimeSnapshot: async () => ({ ...snapshot, state: "ready" }),
    validateWorkspace: async ({ path }) => ({ path }),
    removeRecentWorkspace: async () => ({ workspaces: [] }),
  });
  await controller.initialize();

  const workspace = await controller.openRecent("C:\\work");
  assert.deepEqual(workspace, { path: "C:\\work" });
  assert.deepEqual(controller.getState().selectedWorkspace, workspace);

  await controller.removeRecent("C:\\old");
  assert.deepEqual(controller.getState().recentWorkspaces, []);
});

test("missing Grok remains legible even when a runtime snapshot is unavailable", async () => {
  const missingFailure = {
    code: "executable_unavailable",
    diagnostic: "Grok Build was not found",
    recoverable: true,
  };
  let started = false;
  const controller = createSetupController({
    onEvent: async () => () => {},
    setupStatus: async () => ({
      runtimeAvailable: false,
      executableState: "missing",
      executableSource: null,
      failure: missingFailure,
    }),
    listRecentWorkspaces: async () => ({ workspaces: [] }),
    runtimeSnapshot: async () => {
      throw missingFailure;
    },
    startRuntime: async () => {
      started = true;
      return snapshot;
    },
  });

  await controller.initialize();

  assert.equal(controller.getState().setup?.executableState, "missing");
  assert.equal(controller.getState().failure?.code, "executable_unavailable");
  assert.equal(controller.getState().initializing, false);
  assert.equal(started, false);
});

test("stale runtime events cannot regress an authoritative snapshot", async () => {
  let listener = null;
  const controller = createSetupController({
    onEvent: async (next) => {
      listener = next;
      return () => {};
    },
    setupStatus: async () => setup,
    listRecentWorkspaces: async () => ({ workspaces: [] }),
    runtimeSnapshot: async () => ({
      ...snapshot,
      generation: 2,
      lastSequence: 4,
      state: "ready",
    }),
  });
  await controller.initialize();

  listener({
    generation: 1,
    sequence: 99,
    event: { type: "runtime_state_changed", state: "disconnected" },
  });

  assert.equal(controller.getState().runtimeState, "ready");
});

test("preference failure warns without discarding the selected workspace", async () => {
  const preferenceFailure = {
    code: "preferences_unavailable",
    diagnostic: "Recent workspaces could not be saved",
    recoverable: true,
  };
  let recentCalls = 0;
  const controller = createSetupController({
    onEvent: async () => () => {},
    setupStatus: async () => setup,
    listRecentWorkspaces: async () => {
      recentCalls += 1;
      if (recentCalls === 1) {
        return { workspaces: [] };
      }
      throw preferenceFailure;
    },
    runtimeSnapshot: async () => ({ ...snapshot, state: "ready" }),
    pickWorkspace: async () => ({ path: "C:\\work" }),
  });
  await controller.initialize();

  const selected = await controller.pickWorkspace();

  assert.deepEqual(selected, { path: "C:\\work" });
  assert.deepEqual(controller.getState().selectedWorkspace, selected);
  assert.equal(controller.getState().workspaceFailure?.code, "preferences_unavailable");
});
