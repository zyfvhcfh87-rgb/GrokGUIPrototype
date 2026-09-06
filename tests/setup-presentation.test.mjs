import assert from "node:assert/strict";
import test from "node:test";

import { describeLaunchState, describeWorkspaceList } from "../src/application/setup.ts";

const setup = {
  runtimeAvailable: true,
  executableState: "available",
  executableSource: "user_install",
  failure: null,
};

test("launch presentation makes every setup and runtime state legible", () => {
  assert.equal(
    describeLaunchState({
      setup: null,
      runtimeState: "failed",
      failure: {
        code: "connection_failed",
        diagnostic: "The local setup check could not finish.",
        recoverable: true,
      },
    }).kind,
    "failed",
  );
  assert.deepEqual(
    describeLaunchState({
      setup: { ...setup, runtimeAvailable: false, executableState: "missing" },
      runtimeState: "disconnected",
      failure: {
        code: "executable_unavailable",
        diagnostic: "Grok Build was not found",
        recoverable: true,
      },
    }),
    {
      kind: "missing",
      label: "Grok missing",
      heading: "Install Grok Build to continue",
      detail: "Install Grok Build, then restart this app so it can check again.",
      canRetry: false,
    },
  );

  assert.equal(
    describeLaunchState({
      setup: { ...setup, runtimeAvailable: false, executableState: "invalid" },
      runtimeState: "disconnected",
      failure: {
        code: "executable_unavailable",
        diagnostic: "Configured Grok executable is invalid",
        recoverable: true,
      },
    }).kind,
    "invalid",
  );
  assert.equal(
    describeLaunchState({ setup, runtimeState: "authenticating", failure: null }).kind,
    "authenticating",
  );
  assert.equal(
    describeLaunchState({ setup, runtimeState: "ready", failure: null }).kind,
    "ready",
  );
  const incompatible = describeLaunchState({
    setup,
    runtimeState: "failed",
    failure: {
      code: "unsupported_protocol",
      diagnostic: "ACP protocol v1 was not negotiated",
      recoverable: false,
    },
  });
  assert.equal(incompatible.kind, "incompatible");
  assert.equal(
    incompatible.detail,
    "Update Grok Build to a version that supports ACP v1, then restart the app.",
  );
  assert.equal(
    describeLaunchState({
      setup,
      runtimeState: "failed",
      failure: {
        code: "unsupported_authentication",
        diagnostic: "Runtime advertised no supported authentication method",
        recoverable: true,
      },
    }).kind,
    "unsupported_authentication",
  );
  assert.equal(
    describeLaunchState({
      setup,
      runtimeState: "failed",
      failure: {
        code: "authentication_failed",
        diagnostic: "Authentication failed",
        recoverable: true,
      },
    }).kind,
    "failed",
  );
  assert.equal(
    describeLaunchState({ setup, runtimeState: "disconnected", failure: null }).kind,
    "disconnected",
  );
});

test("workspace presentation distinguishes empty, ready, and stale recents", () => {
  assert.deepEqual(describeWorkspaceList([]), {
    kind: "empty",
    availableCount: 0,
    staleCount: 0,
  });
  assert.deepEqual(
    describeWorkspaceList([
      { path: "C:\\work", available: true, lastSessionId: null },
      { path: "C:\\old", available: false, lastSessionId: null },
    ]),
    { kind: "mixed", availableCount: 1, staleCount: 1 },
  );
});
