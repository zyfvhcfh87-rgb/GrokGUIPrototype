import assert from "node:assert/strict";
import test from "node:test";

import {
  APP_IDENTITY,
  buildCompatibilityReport,
  compatibilityReportIsSafe,
  serializeCompatibilityReport,
} from "../src/application/compatibility.ts";

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
  models: {
    currentModelId: "fixture-fast",
    availableModels: [{ modelId: "fixture-fast", name: "Fixture Fast", description: null, agentType: null, reasoningEffort: null, reasoningEfforts: [], supportsReasoningEffort: null, totalContextTokens: null, truncated: false }],
    truncated: false,
  },
  truncated: false,
};

test("compatibility reports are built from negotiated data and omit private material", () => {
  const report = buildCompatibilityReport({
    generatedAt: "2030-01-01T00:00:00.000Z",
    setup: {
      runtimeAvailable: true,
      executableState: "available",
      executableSource: "user_install",
      failure: null,
    },
    snapshot: { state: "ready", capabilities },
  });
  const text = serializeCompatibilityReport(report);
  assert.equal(report.app.version, APP_IDENTITY.version);
  assert.equal(report.runtime.protocolVersion, 1);
  assert.equal(report.runtime.agentProduct, "grok_build");
  assert.equal(report.sessions.list, true);
  assert.equal(report.models.count, 1);
  assert.equal(
    report.features.find((feature) => feature.id === "cancel")?.status,
    "advertised",
  );
  assert.ok(compatibilityReportIsSafe(text));
  assert.doesNotMatch(text, /session-|auth\.json|stderr|C:\\|\/Users\/|\/home\//u);
  assert.doesNotMatch(text, /prompt text|secret|fixture-fast/iu);
});

test("unavailable capabilities are explained rather than invented", () => {
  const report = buildCompatibilityReport({
    generatedAt: "2030-01-01T00:00:00.000Z",
    setup: {
      runtimeAvailable: true,
      executableState: "available",
      executableSource: "path",
      failure: null,
    },
    snapshot: {
      state: "ready",
      capabilities: {
        ...capabilities,
        sessions: { ...capabilities.sessions, cancel: false, list: false },
        models: null,
      },
    },
  });
  assert.equal(report.features.find((feature) => feature.id === "cancel")?.status, "not_advertised");
  assert.match(report.features.find((feature) => feature.id === "cancel")?.note ?? "", /not advertised/i);
  assert.equal(report.models.advertised, false);
});

test("a missing snapshot stays not_reported instead of assuming Grok behavior", () => {
  const report = buildCompatibilityReport({
    generatedAt: "2030-01-01T00:00:00.000Z",
    setup: null,
    snapshot: null,
  });
  assert.equal(report.runtime.state, "unknown");
  assert.ok(report.features.every((feature) => feature.status === "not_reported" || feature.id === "protocol"));
});