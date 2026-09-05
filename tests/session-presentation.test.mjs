import assert from "node:assert/strict";
import test from "node:test";

import {
  describeSessionList,
  describeSessionStatus,
  sameWorkspacePath,
  sessionBelongsToWorkspace,
} from "../src/application/sessions.ts";

const capabilities = {
  create: true,
  prompt: true,
  cancel: true,
  list: true,
  load: true,
  resume: true,
  close: true,
};

test("workspace path comparison treats Windows drive-letter case as the same folder", () => {
  assert.equal(sameWorkspacePath("C:\\work", "C:\\work"), true);
  assert.equal(sameWorkspacePath("C:\\work\\", "C:\\work"), true);
  assert.equal(sameWorkspacePath("C:\\work", "c:\\work"), true);
  assert.equal(sameWorkspacePath("C:\\work", "C:\\other"), false);
  assert.equal(sameWorkspacePath("/Users/lyn/a", "/Users/lyn/b"), false);
});

test("session belonging is decided by workspace path, not session identity", () => {
  assert.equal(
    sessionBelongsToWorkspace({ workspace: "C:\\work" }, "C:\\work"),
    true,
  );
  assert.equal(
    sessionBelongsToWorkspace({ workspace: "C:\\other" }, "C:\\work"),
    false,
  );
});

test("session list presentation covers idle, loading, empty, ready, stale, and failed", () => {
  assert.equal(
    describeSessionList({
      workspace: null,
      runtimeState: "ready",
      capabilities,
      listKind: "idle",
      sessionCount: 0,
      failure: null,
    }).kind,
    "idle",
  );
  assert.equal(
    describeSessionList({
      workspace: "C:\\work",
      runtimeState: "ready",
      capabilities,
      listKind: "loading",
      sessionCount: 0,
      failure: null,
    }).kind,
    "loading",
  );
  assert.deepEqual(
    describeSessionList({
      workspace: "C:\\work",
      runtimeState: "ready",
      capabilities,
      listKind: "empty",
      sessionCount: 0,
      failure: null,
    }).kind,
    "empty",
  );
  const ready = describeSessionList({
    workspace: "C:\\work",
    runtimeState: "ready",
    capabilities,
    listKind: "ready",
    sessionCount: 2,
    failure: null,
  });
  assert.equal(ready.kind, "ready");
  assert.equal(ready.canCreate, true);
  assert.equal(ready.canOpen, true);
  assert.equal(ready.canClose, true);
  assert.equal(
    describeSessionList({
      workspace: "C:\\work",
      runtimeState: "ready",
      capabilities,
      listKind: "stale",
      sessionCount: 1,
      failure: null,
    }).kind,
    "stale",
  );
  const failed = describeSessionList({
    workspace: "C:\\work",
    runtimeState: "ready",
    capabilities,
    listKind: "failed",
    sessionCount: 0,
    failure: {
      code: "protocol_request_failed",
      diagnostic: "Sessions for this workspace could not be loaded.",
      recoverable: true,
    },
  });
  assert.equal(failed.kind, "failed");
  assert.equal(failed.canRetry, true);
  assert.equal(
    describeSessionList({
      workspace: "C:\\work",
      runtimeState: "disconnected",
      capabilities,
      listKind: "unavailable",
      sessionCount: 0,
      failure: null,
    }).kind,
    "unavailable",
  );
});

test("session status labels stay distinct for selected, closed, stale, and failed", () => {
  assert.equal(describeSessionStatus("selected"), "Selected");
  assert.equal(describeSessionStatus("closed"), "Closed");
  assert.equal(describeSessionStatus("stale"), "Stale");
  assert.equal(describeSessionStatus("failed"), "Failed");
});
