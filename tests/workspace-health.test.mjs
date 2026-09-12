import assert from "node:assert/strict";
import test from "node:test";

import {
  WORKSPACE_CHANGE_DISCLAIMER,
  createWorkspaceHealthController,
  describeChangeStatus,
  describeRuntimeHealth,
  describeWorkspaceChanges,
} from "../src/application/workspace-health.ts";

const cleanChanges = {
  kind: "repository",
  attributableToSession: false,
  entries: [],
  truncated: false,
  omittedEntryCount: 0,
  omittedLineCount: 0,
};

const dirtyChanges = {
  kind: "repository",
  attributableToSession: true,
  entries: [
    {
      path: "src/lib.rs",
      previousPath: null,
      status: "modified",
      content: "text",
      diff: "@@ -1 +1 @@",
      truncated: false,
    },
  ],
  truncated: false,
  omittedEntryCount: 0,
  omittedLineCount: 0,
};

test("workspace changes never claim session attribution", () => {
  const presentation = describeWorkspaceChanges({
    workspace: "C:\\work",
    changes: dirtyChanges,
    failure: null,
    loading: false,
  });
  assert.equal(presentation.disclaimer, WORKSPACE_CHANGE_DISCLAIMER);
  assert.equal(presentation.kind, "repository");
  assert.equal(describeChangeStatus("modified"), "Modified");
});

test("non-repositories, truncation, and path errors have dedicated states", () => {
  assert.equal(
    describeWorkspaceChanges({
      workspace: "C:\\work",
      changes: {
        ...cleanChanges,
        kind: "not_a_repository",
      },
      failure: null,
      loading: false,
    }).kind,
    "not_a_repository",
  );
  const truncated = describeWorkspaceChanges({
    workspace: "C:\\work",
    changes: {
      ...dirtyChanges,
      truncated: true,
      omittedEntryCount: 3,
      omittedLineCount: 40,
    },
    failure: null,
    loading: false,
  });
  assert.equal(truncated.truncated, true);
  assert.equal(truncated.omittedEntryCount, 3);
  const failed = describeWorkspaceChanges({
    workspace: "C:\\work",
    changes: null,
    failure: {
      code: "invalid_workspace",
      diagnostic: "select an existing workspace directory",
      recoverable: true,
    },
    loading: false,
  });
  assert.equal(failed.kind, "failed");
  assert.equal(failed.canRefresh, true);
});

test("runtime health recover stays available after crash loops", () => {
  const health = describeRuntimeHealth({
    diagnostics: {
      state: "failed",
      workerRunning: false,
      consecutiveFailures: 3,
      lastFailure: {
        code: "connection_failed",
        diagnostic: "runtime process ended",
        recoverable: true,
      },
      stderrLines: 2,
      stderrBytes: 40,
      stderrTruncatedLines: 0,
      stderrReadErrors: 0,
      processContainment: "direct_child",
    },
    failure: null,
    loading: false,
  });
  assert.equal(health.canRecover, true);
  assert.equal(health.consecutiveFailures, 3);
  assert.match(health.detail, /Repeated process failure/);
});

test("workspace health controller inspects the selected folder and forces no attribution", async () => {
  const calls = [];
  const controller = createWorkspaceHealthController({
    inspectWorkspaceChanges: async (request) => {
      calls.push(request);
      return dirtyChanges;
    },
    runtimeDiagnostics: async () => ({
      state: "ready",
      workerRunning: true,
      consecutiveFailures: 0,
      lastFailure: null,
      stderrLines: 0,
      stderrBytes: 0,
      stderrTruncatedLines: 0,
      stderrReadErrors: 0,
      processContainment: "windows_job",
    }),
  });

  await controller.setWorkspace({ path: "C:\\work" });
  assert.deepEqual(calls, [{ path: "C:\\work" }]);
  assert.equal(controller.getState().changes?.attributableToSession, false);
  await controller.refreshDiagnostics();
  assert.equal(controller.getState().diagnostics?.workerRunning, true);
});
