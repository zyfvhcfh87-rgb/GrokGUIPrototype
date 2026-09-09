import assert from "node:assert/strict";
import test from "node:test";
import { createInteractionController } from "../src/application/interaction-controller.ts";
import { pendingInteractions, responseKey, permissionActions, validPermissionScope, elicitationAnswer, validElicitationControl } from "../src/application/interactions.ts";
import { initialApplicationState, reduceApplicationEvent } from "../src/application/state.ts";
import { createApplicationBridge } from "../src/application/bridge.ts";
import { createConversationController } from "../src/application/conversation-controller.ts";
import { createConversationFixtureTransport, FIXTURE_SESSION_ID, FIXTURE_WORKSPACE } from "../src/application/conversation-fixture.ts";

const permission = (id = "permission-1") => ({
  type: "permission_requested", sessionId: "session-1", interactionId: id,
  title: "Delete fixture output", consequence: "Deletes only the listed fixture file.",
  scope: { type: "command", command: "rm -- /workspace/out.txt", workingDirectory: "/workspace", affectedPaths: ["/workspace/out.txt"] },
  availableDecisions: ["allow_once", "allow_always", "deny_once", "deny_always"],
});
const textControl = { type: "text", fieldId: "label", label: "Label", sensitive: true, placeholder: null, minLength: 1, maxLength: 64 };
const elicitation = (control = textControl, sessionId = "session-1") => ({ type: "elicitation_requested", sessionId, interactionId: "form-1", prompt: "Enter a label", control });

function harness() {
  let application = initialApplicationState(1);
  let context = { application, sessionId: "session-1", epoch: 1, blocked: false };
  const calls = [];
  let reply = async () => ({ acknowledged: true });
  const controller = createInteractionController({
    respondToPermission: request => { calls.push(request); return reply(); },
    respondToElicitation: request => { calls.push(request); return reply(); },
  }, () => context);
  const emit = event => {
    application = reduceApplicationEvent(application, { generation: application.generation, sequence: application.lastSequence + 1, event });
    context = { ...context, application };
  };
  emit({ type: "runtime_state_changed", state: "ready" });
  emit({ type: "session_activated", sessionId: "session-1" });
  emit({ type: "session_state_changed", sessionId: "session-1", state: "working" });
  return { controller, calls, emit, target: () => pendingInteractions(context)[0], context: () => context,
    change: patch => { context = { ...context, ...patch }; }, reply: fn => { reply = fn; } };
}

for (const decision of ["allow_once", "allow_always", "deny_once", "deny_always", "cancel"]) {
  test(`permission ${decision} sends exactly the advertised decision once`, async () => {
    const h = harness(); h.emit(permission()); const target = h.target();
    assert.equal(await h.controller.respondPermission(target, decision), true);
    assert.equal(await h.controller.respondPermission(target, decision), false);
    assert.deepEqual(h.calls, [{ interactionId: "permission-1", decision }]);
  });
}

test("missing, relative, malformed and ambiguous scope cannot approve", async () => {
  for (const scope of [null, { type: "other" }, { type: "tool", toolName: "shell" },
    { ...permission().scope, command: "   " }, { ...permission().scope, workingDirectory: null },
    { ...permission().scope, workingDirectory: "relative" }, { ...permission().scope, affectedPaths: ["relative"] }]) {
    assert.equal(validPermissionScope(scope), false);
    const h = harness(); h.emit({ ...permission(), scope });
    assert.equal(await h.controller.respondPermission(h.target(), "allow_once"), false);
    assert.equal(await h.controller.respondPermission(h.target(), "cancel"), true);
    assert.equal(h.calls.length, 1);
  }
});

test("unsupported and duplicate decisions fail closed; approvals are never invented", () => {
  for (const availableDecisions of [[], ["allow_once", "allow_once"], ["allow_once", "future_decision"]]) {
    assert.deepEqual(permissionActions({ ...permission(), availableDecisions }).map(action => action.decision), ["cancel"]);
  }
  assert.deepEqual(permissionActions({ ...permission(), availableDecisions: ["deny_once"] }).map(action => action.decision), ["deny_once", "cancel"]);
  assert.equal(permissionActions(permission())[0].decision, "deny_once");
  assert.equal(validPermissionScope({ ...permission().scope, workingDirectory: "C:\\work", affectedPaths: ["C:\\work\\out.txt"] }), true);
});

test("typed form validation covers bounds, unicode, booleans, choices, and arrays", () => {
  assert.equal(elicitationAnswer(textControl, "").decision, null);
  assert.equal(elicitationAnswer(textControl, "x".repeat(65)).decision, null);
  assert.equal(elicitationAnswer(textControl, "line\nbreak").decision, null);
  assert.equal(elicitationAnswer({ ...textControl, maxLength: 16384 }, "😺".repeat(5000)).decision, null);
  assert.equal(elicitationAnswer({ ...textControl, minLength: 1, maxLength: 1 }, "😺").error, null);
  const boolean = { type: "confirmation", fieldId: "confirm", label: null };
  assert.equal(elicitationAnswer(boolean, null).decision, null);
  assert.deepEqual(elicitationAnswer(boolean, false).decision.value.content.confirm, { type: "boolean", value: false });
  const choice = { type: "choice", fieldId: "color", label: null, options: ["red", "blue"], multiple: false, truncated: false };
  assert.equal(elicitationAnswer(choice, "green").decision, null);
  assert.equal(elicitationAnswer(choice, "red").error, null);
  assert.equal(elicitationAnswer({ ...choice, multiple: true }, ["red", "red"]).decision, null);
  assert.deepEqual(elicitationAnswer({ ...choice, multiple: true }, ["red", "blue"]).decision.value.content.color.value, ["red", "blue"]);
  for (const control of [{ type: "other" }, { ...choice, truncated: true }, { ...choice, options: ["red", "red"] }, { ...textControl, minLength: 100 }, { ...textControl, maxLength: undefined }]) {
    assert.equal(validElicitationControl(control), false);
  }
});

test("elicitation accept and cancel send only typed values, with no persisted input", async () => {
  const h = harness(); h.emit(elicitation());
  assert.equal(await h.controller.respondElicitation(h.target(), "secret-example"), true);
  assert.deepEqual(h.calls[0], { interactionId: "form-1", decision: { type: "accept", value: { content: { label: { type: "string", value: "secret-example" } } } } });
  assert.doesNotMatch(JSON.stringify(h.controller.getState()), /secret-example/);
  assert.doesNotMatch(JSON.stringify(h.context().application), /secret-example/);
  const cancelled = harness(); cancelled.emit(elicitation({ type: "other" }, null));
  assert.equal(await cancelled.controller.cancelElicitation(cancelled.target()), true);
  assert.deepEqual(cancelled.calls[0].decision, { type: "cancel" });
});

for (const state of ["cancelling", "completed", "closed", "failed"]) {
  test(`${state} expires permission and elicitation callbacks`, async () => {
    for (const request of [permission(), elicitation()]) {
      const h = harness(); h.emit(request); const target = h.target();
      h.emit({ type: "session_state_changed", sessionId: "session-1", state });
      assert.equal(await (target.kind === "permission" ? h.controller.respondPermission(target, "allow_once") : h.controller.respondElicitation(target, "answer")), false);
      assert.equal(h.calls.length, 0);
    }
  });
}

test("navigation, restart, event gaps and resync block stale responses", async () => {
  for (const change of [h => h.change({ blocked: true }), h => h.change({ sessionId: "other" }), h => h.change({ epoch: 2 }),
    h => h.change({ application: initialApplicationState(2) }),
    h => h.change({ application: { ...h.context().application, needsResync: true } }),
    h => h.change({ application: { ...h.context().application, pendingEventCount: 1 } })]) {
    const h = harness(); h.emit(permission()); const target = h.target(); change(h);
    assert.equal(await h.controller.respondPermission(target, "allow_once"), false);
    assert.equal(h.calls.length, 0);
  }
});

test("orphaned requests cannot create sessions; duplicate and resolved requests cannot return", () => {
  const h = harness(); h.emit({ ...permission(), sessionId: "unknown" });
  assert.equal(h.context().application.sessions.unknown, undefined);
  h.emit(permission("valid")); const original = h.target();
  h.emit({ ...permission("valid"), scope: { type: "other" } });
  assert.equal(h.target().request, original.request);
  h.emit({ type: "interaction_resolved", interactionId: "valid", kind: "permission" });
  h.emit(permission("valid")); assert.equal(h.target(), undefined);
});

test("in-flight duplicate is blocked and late transport errors cannot leak into a new context", async () => {
  const h = harness(); h.emit(elicitation()); const target = h.target(); let reject;
  h.reply(() => new Promise((_, fail) => { reject = fail; }));
  const pending = h.controller.respondElicitation(target, "secret-example");
  assert.equal(await h.controller.respondElicitation(target, "secret-example"), false);
  h.change({ application: initialApplicationState(2) }); h.controller.reconcile();
  reject(new Error("secret-example")); await pending;
  assert.deepEqual(h.controller.getState(), {});
  assert.equal(h.calls.length, 1);
});

test("uncertain delivery stays disabled and shows only a static error", async () => {
  const h = harness(); h.emit(elicitation());
  h.reply(async () => { throw new Error("secret-example"); });
  const target = h.target(); assert.equal(await h.controller.respondElicitation(target, "secret-example"), false);
  assert.equal(await h.controller.respondElicitation(target, "secret-example"), false);
  assert.equal(h.controller.getState()[responseKey(target)].phase, "failed");
  assert.doesNotMatch(JSON.stringify(h.controller.getState()), /secret-example/);
});

test("a temporary gap or navigation cannot release an in-flight submission lock", async () => {
  const h = harness(); h.emit(permission()); let complete;
  h.reply(() => new Promise(resolve => { complete = resolve; }));
  const first = h.controller.respondPermission(h.target(), "allow_once");
  h.change({ blocked: true }); h.controller.reconcile();
  h.change({ blocked: false, epoch: 2 }); h.controller.reconcile();
  assert.equal(await h.controller.respondPermission(h.target(), "allow_once"), false);
  complete({ acknowledged: true }); await first;
  assert.equal(await h.controller.respondPermission(h.target(), "allow_once"), false);
  assert.equal(h.calls.length, 1);
});

test("the development fixture exercises the real bridge, responses and return to ready", async () => {
  const bridge = createApplicationBridge(createConversationFixtureTransport(true));
  const conversation = createConversationController(bridge);
  await conversation.initialize();
  const session = await bridge.resumeSession({ sessionId: FIXTURE_SESSION_ID, workspace: FIXTURE_WORKSPACE });
  conversation.setSession(session);
  const read = () => ({ application: conversation.getState().application, sessionId: FIXTURE_SESSION_ID, epoch: conversation.getState().interactionEpoch, blocked: false });
  const controller = createInteractionController(bridge, read);
  assert.equal(pendingInteractions(read()).length, 3);
  assert.equal(await controller.respondPermission(pendingInteractions(read())[0], "deny_once"), true);
  assert.equal(await controller.respondElicitation(pendingInteractions(read())[0], ""), false);
  assert.equal(await controller.respondElicitation(pendingInteractions(read())[0], "Fixture result"), true);
  assert.equal(await controller.respondElicitation(pendingInteractions(read())[0], "Markdown"), true);
  assert.equal(pendingInteractions(read()).length, 0);
  assert.equal(conversation.getState().application.sessions[FIXTURE_SESSION_ID].state, "ready");
  conversation.dispose();
});
