import assert from "node:assert/strict";
import test from "node:test";

import { describeOnboarding, nextOnboardingStep } from "../src/application/onboarding.ts";
import { describeRecovery } from "../src/application/setup.ts";

const setup = {
  runtimeAvailable: true,
  executableState: "available",
  executableSource: "user_install",
  failure: null,
};

test("onboarding can be finished without a workspace and skip stays independent of runtime state", () => {
  const presentation = describeOnboarding({
    step: "compatibility",
    setup,
    runtimeState: "ready",
    failure: null,
    selectedWorkspace: null,
    capabilities: {
      protocolVersion: 1,
      agent: { product: "grok_build", version: "fixture" },
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
    },
  });
  assert.equal(presentation.canFinish, true);
  assert.equal(nextOnboardingStep("welcome"), "detect");
});

test("recovery copy distinguishes setup, authentication, protocol, and transient failure", () => {
  assert.equal(
    describeRecovery({
      setup,
      runtimeState: "failed",
      failure: { code: "unsupported_protocol", diagnostic: "no", recoverable: false },
      workspaceFailure: null,
      sessionFailure: null,
    })?.kind,
    "protocol",
  );
  assert.equal(
    describeRecovery({
      setup,
      runtimeState: "failed",
      failure: { code: "authentication_failed", diagnostic: "sign-in", recoverable: true },
      workspaceFailure: null,
      sessionFailure: null,
    })?.kind,
    "authentication",
  );
  assert.equal(
    describeRecovery({
      setup: { ...setup, runtimeAvailable: false, executableState: "missing" },
      runtimeState: "disconnected",
      failure: { code: "executable_unavailable", diagnostic: "missing", recoverable: true },
      workspaceFailure: null,
      sessionFailure: null,
    })?.kind,
    "setup",
  );
  assert.equal(
    describeRecovery({
      setup,
      runtimeState: "disconnected",
      failure: { code: "connection_failed", diagnostic: "lost", recoverable: true },
      workspaceFailure: null,
      sessionFailure: null,
    })?.kind,
    "transient",
  );
  assert.equal(
    describeRecovery({
      setup,
      runtimeState: "ready",
      failure: null,
      workspaceFailure: null,
      sessionFailure: { code: "session_unavailable", diagnostic: "gone", recoverable: true },
    })?.kind,
    "stale_session",
  );
  assert.equal(
    describeRecovery({
      setup,
      runtimeState: "ready",
      failure: null,
      workspaceFailure: { code: "workspace_unavailable", diagnostic: "gone", recoverable: true },
      sessionFailure: null,
    })?.kind,
    "stale_workspace",
  );
});