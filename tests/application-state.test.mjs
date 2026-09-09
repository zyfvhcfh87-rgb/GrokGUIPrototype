import assert from "node:assert/strict";
import test from "node:test";

import {
  createApplicationStore,
  initialApplicationState,
  reduceApplicationEvent,
} from "../src/application/state.ts";

const runtimeEvent = (generation, sequence, state) => ({
  generation,
  sequence,
  event: { type: "runtime_state_changed", state },
});

test("out-of-order events wait for the missing sequence and then reduce in order", () => {
  let state = initialApplicationState();

  state = reduceApplicationEvent(state, runtimeEvent(1, 2, "ready"));
  assert.equal(state.runtime.state, "disconnected");
  assert.equal(state.lastSequence, 0);
  assert.equal(state.pendingEventCount, 1);

  state = reduceApplicationEvent(state, runtimeEvent(1, 1, "connecting"));
  assert.equal(state.runtime.state, "ready");
  assert.equal(state.lastSequence, 2);
  assert.equal(state.pendingEventCount, 0);
});

test("duplicate delivery is idempotent even when the duplicate payload differs", () => {
  let state = reduceApplicationEvent(
    initialApplicationState(),
    runtimeEvent(1, 1, "connecting"),
  );

  state = reduceApplicationEvent(state, runtimeEvent(1, 1, "ready"));

  assert.equal(state.runtime.state, "connecting");
  assert.equal(state.lastSequence, 1);
});

test("events for a closed session cannot resurrect stale conversation state", () => {
  let state = initialApplicationState();
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 1,
    event: {
      type: "session_state_changed",
      sessionId: "session-old",
      state: "ready",
    },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 2,
    event: {
      type: "session_state_changed",
      sessionId: "session-old",
      state: "closed",
    },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 3,
    event: {
      type: "message_chunk_received",
      sessionId: "session-old",
      messageId: "late-message",
      text: "stale",
      truncated: false,
    },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 4,
    event: {
      type: "session_state_changed",
      sessionId: "session-old",
      state: "working",
    },
  });

  assert.equal(state.sessions["session-old"].state, "closed");
  assert.deepEqual(state.sessions["session-old"].messages, {});

  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 5,
    event: { type: "session_activated", sessionId: "session-old" },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 6,
    event: {
      type: "session_state_changed",
      sessionId: "session-old",
      state: "ready",
    },
  });

  assert.equal(state.sessions["session-old"].state, "ready");
});

test("a newer runtime generation atomically clears sessions and rejects old events", () => {
  let state = initialApplicationState();
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 1,
    event: {
      type: "session_state_changed",
      sessionId: "session-before-restart",
      state: "working",
    },
  });
  state = reduceApplicationEvent(state, runtimeEvent(2, 1, "connecting"));
  state = reduceApplicationEvent(state, runtimeEvent(1, 2, "ready"));

  assert.equal(state.generation, 2);
  assert.equal(state.runtime.state, "connecting");
  assert.deepEqual(state.sessions, {});
});

test("the store publishes deterministic state changes and ignores duplicates", () => {
  const store = createApplicationStore();
  let notifications = 0;
  const unsubscribe = store.subscribe(() => {
    notifications += 1;
  });

  store.dispatch(runtimeEvent(1, 1, "connecting"));
  store.dispatch(runtimeEvent(1, 1, "ready"));
  unsubscribe();
  store.dispatch(runtimeEvent(1, 2, "ready"));

  assert.equal(store.getState().runtime.state, "ready");
  assert.equal(notifications, 1);
});

test("runtime-scoped elicitation is retained and removed when resolved", () => {
  let state = reduceApplicationEvent(
    initialApplicationState(),
    runtimeEvent(1, 1, "authenticating"),
  );
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 2,
    event: {
      type: "elicitation_requested",
      sessionId: null,
      interactionId: "global-question",
      prompt: "Continue?",
      control: {
        type: "confirmation",
        fieldId: "continue",
        label: null,
      },
    },
  });

  assert.equal(
    state.runtime.elicitations["global-question"].prompt,
    "Continue?",
  );
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 3,
    event: {
      type: "interaction_resolved",
      interactionId: "global-question",
      kind: "elicitation",
    },
  });
  assert.deepEqual(state.runtime.elicitations, {});
});

test("automatic session and runtime cancellation clears orphaned interactions", () => {
  const permission = {
    type: "permission_requested",
    sessionId: "session-1",
    interactionId: "permission-1",
    title: "Run",
    consequence: null,
    scope: { type: "tool", toolName: "fixture" },
    availableDecisions: ["allow_once", "deny_once"],
  };
  const elicitation = {
    type: "elicitation_requested",
    sessionId: "session-1",
    interactionId: "elicitation-1",
    prompt: "Continue?",
    control: { type: "confirmation", fieldId: "continue", label: null },
  };
  let state = reduceApplicationEvent(
    initialApplicationState(),
    runtimeEvent(1, 1, "ready"),
  );
  state = [{ type: "session_activated", sessionId: "session-1" }, permission, elicitation].reduce(
    (current, event, index) =>
      reduceApplicationEvent(current, { generation: 1, sequence: index + 2, event }),
    state,
  );
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 5,
    event: { type: "interactions_cleared", sessionId: "session-1" },
  });
  assert.deepEqual(state.sessions["session-1"].permissions, {});
  assert.deepEqual(state.sessions["session-1"].elicitations, {});

  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 6,
    event: {
      type: "session_state_changed",
      sessionId: "session-1",
      state: "cancelling",
    },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 7,
    event: permission,
  });
  assert.deepEqual(state.sessions["session-1"].permissions, {});

  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 8,
    event: {
      type: "elicitation_requested",
      sessionId: null,
      interactionId: "runtime-question",
      prompt: "Authenticate?",
      control: { type: "confirmation", fieldId: "authenticate", label: null },
    },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 9,
    event: { type: "runtime_state_changed", state: "disconnected" },
  });
  assert.deepEqual(state.runtime.elicitations, {});
});

test("runtime failure and disconnect reject late session interactions", () => {
  for (const runtimeState of ["failed", "disconnected"]) {
    let state = reduceApplicationEvent(
      initialApplicationState(),
      runtimeEvent(1, 1, "ready"),
    );
    state = reduceApplicationEvent(state, {
      generation: 1,
      sequence: 2,
      event: { type: "session_activated", sessionId: "session-1" },
    });
    state = reduceApplicationEvent(state, {
      generation: 1,
      sequence: 3,
      event: { type: "runtime_state_changed", state: runtimeState },
    });
    state = reduceApplicationEvent(state, {
      generation: 1,
      sequence: 4,
      event: {
        type: "permission_requested",
        sessionId: "session-1",
        interactionId: "late-permission",
        title: "Run",
        consequence: null,
        scope: { type: "tool", toolName: "fixture" },
        availableDecisions: ["deny_once"],
      },
    });
    state = reduceApplicationEvent(state, {
      generation: 1,
      sequence: 5,
      event: {
        type: "elicitation_requested",
        sessionId: "session-1",
        interactionId: "late-elicitation",
        prompt: "Continue?",
        control: { type: "confirmation", fieldId: "continue", label: null },
      },
    });

    assert.deepEqual(state.sessions["session-1"].permissions, {});
    assert.deepEqual(state.sessions["session-1"].elicitations, {});
  }
});

test("every structured application event updates its owned view state", () => {
  const events = [
    { type: "session_activated", sessionId: "session-1" },
    {
      type: "user_message_chunk_received",
      sessionId: "session-1",
      messageId: "user-1",
      text: "hello",
      truncated: false,
    },
    {
      type: "message_chunk_received",
      sessionId: "session-1",
      messageId: "assistant-1",
      text: "hi",
      truncated: false,
    },
    {
      type: "thought_chunk_received",
      sessionId: "session-1",
      thoughtId: "thought-1",
      text: "checking",
      truncated: false,
    },
    {
      type: "tool_call_changed",
      sessionId: "session-1",
      callId: "tool-1",
      title: "Inspect",
      kind: "tool",
      status: "running",
      detail: null,
    },
    {
      type: "permission_requested",
      sessionId: "session-1",
      interactionId: "permission-1",
      title: "Run tests",
      consequence: "Writes build artifacts",
      scope: { type: "command", command: "cargo test", workingDirectory: "C:\\work" },
      availableDecisions: ["allow_once", "deny_once"],
    },
    {
      type: "elicitation_requested",
      sessionId: "session-1",
      interactionId: "elicitation-1",
      prompt: "Pick one",
      control: {
        type: "choice",
        fieldId: "choice",
        label: null,
        options: ["A", "B"],
        multiple: false,
        truncated: false,
      },
    },
    {
      type: "plan_changed",
      sessionId: "session-1",
      entries: [{ id: "step-1", title: "Test", description: null, status: "in_progress" }],
      truncated: false,
    },
    {
      type: "usage_changed",
      sessionId: "session-1",
      usage: {
        inputTokens: 10,
        outputTokens: 5,
        cachedInputTokens: 2,
        totalTokens: 15,
        contextWindowTokens: 1000,
      },
    },
    { type: "session_metadata_changed", sessionId: "session-1" },
    {
      type: "available_commands_changed",
      sessionId: "session-1",
      commands: [{ name: "review", description: "Review", acceptsInput: false }],
      truncated: false,
    },
    { type: "session_mode_changed", sessionId: "session-1", currentModeId: "plan" },
    {
      type: "session_config_options_changed",
      sessionId: "session-1",
      configOptions: [
        {
          id: "effort",
          name: "Effort",
          description: null,
          kind: { type: "boolean", currentValue: true },
        },
      ],
      truncated: false,
    },
    {
      type: "session_info_changed",
      sessionId: "session-1",
      title: { state: "value", value: "Fixture" },
      updatedAt: { state: "cleared" },
    },
    {
      type: "runtime_extension_invalidated",
      area: "session",
      sessionId: "session-1",
    },
    { type: "extension_observed", classification: "unknown" },
    { type: "interaction_resolved", interactionId: "permission-1", kind: "permission" },
    { type: "interaction_resolved", interactionId: "elicitation-1", kind: "elicitation" },
    { type: "interactions_cleared", sessionId: "session-1" },
    { type: "runtime_extension_invalidated", area: "models", sessionId: null },
    { type: "runtime_failed", diagnostic: "safe failure", recoverable: true },
  ];

  const state = events.reduce(
    (current, event, index) =>
      reduceApplicationEvent(current, { generation: 1, sequence: index + 1, event }),
    initialApplicationState(),
  );
  const session = state.sessions["session-1"];

  assert.equal(session.messages["user:user-1"].text, "hello");
  assert.equal(session.messages["assistant:assistant-1"].text, "hi");
  assert.equal(session.thoughts["thought-1"].text, "checking");
  assert.equal(session.toolCalls["tool-1"].status, "running");
  assert.deepEqual(session.timeline, [
    { kind: "user_message", id: "user:user-1" },
    { kind: "assistant_message", id: "assistant:assistant-1" },
    { kind: "thought", id: "thought-1" },
    { kind: "tool", id: "tool-1" },
  ]);
  assert.deepEqual(session.permissions, {});
  assert.deepEqual(session.elicitations, {});
  assert.equal(session.plan[0].status, "in_progress");
  assert.equal(session.usage.totalTokens, 15);
  assert.equal(session.metadataRevision, 1);
  assert.equal(session.availableCommands[0].name, "review");
  assert.equal(session.currentModeId, "plan");
  assert.equal(session.configOptions[0].id, "effort");
  assert.equal(session.title, "Fixture");
  assert.deepEqual(session.invalidatedAreas, ["session"]);
  assert.deepEqual(state.runtime.invalidatedAreas, ["models"]);
  assert.equal(state.observedExtensions.unknown, 1);
  assert.equal(state.runtime.failure.diagnostic, "safe failure");
});

test("message and thought chunks append without duplicating timeline items", () => {
  let state = reduceApplicationEvent(initialApplicationState(), {
    generation: 1,
    sequence: 1,
    event: { type: "session_activated", sessionId: "session-1" },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 2,
    event: {
      type: "message_chunk_received",
      sessionId: "session-1",
      messageId: "assistant-1",
      text: "Hel",
      truncated: false,
    },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 3,
    event: {
      type: "message_chunk_received",
      sessionId: "session-1",
      messageId: "assistant-1",
      text: "lo",
      truncated: false,
    },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 4,
    event: {
      type: "thought_chunk_received",
      sessionId: "session-1",
      thoughtId: "thought-1",
      text: "one ",
      truncated: false,
    },
  });
  state = reduceApplicationEvent(state, {
    generation: 1,
    sequence: 5,
    event: {
      type: "thought_chunk_received",
      sessionId: "session-1",
      thoughtId: "thought-1",
      text: "two",
      truncated: true,
    },
  });

  const session = state.sessions["session-1"];
  assert.equal(session.messages["assistant:assistant-1"].text, "Hello");
  assert.equal(session.thoughts["thought-1"].text, "one two");
  assert.equal(session.thoughts["thought-1"].truncated, true);
  assert.deepEqual(
    session.timeline.filter((item) => item.kind !== "tool"),
    [
      { kind: "assistant_message", id: "assistant:assistant-1" },
      { kind: "thought", id: "thought-1" },
    ],
  );
});

test("tool cards update in place across running, completed, and failed statuses", () => {
  let state = reduceApplicationEvent(initialApplicationState(), {
    generation: 1,
    sequence: 1,
    event: { type: "session_activated", sessionId: "session-1" },
  });
  const change = (sequence, status, detail) =>
    reduceApplicationEvent(state, {
      generation: 1,
      sequence,
      event: {
        type: "tool_call_changed",
        sessionId: "session-1",
        callId: "term-1",
        title: "List",
        kind: "terminal_command",
        status,
        detail,
      },
    });

  state = change(2, "running", "partial");
  state = change(3, "completed", "done");
  state = change(4, "failed", "nope");

  const session = state.sessions["session-1"];
  assert.equal(session.toolCalls["term-1"].status, "failed");
  assert.equal(session.toolCalls["term-1"].detail, "nope");
  assert.deepEqual(session.timeline, [{ kind: "tool", id: "term-1" }]);
});
