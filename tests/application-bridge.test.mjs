import assert from "node:assert/strict";
import test from "node:test";

import { createApplicationBridge } from "../src/application/bridge.ts";

test("typed bridge maps intents to exact narrow commands and one event channel", async () => {
  const invocations = [];
  let subscribedEvent = null;
  let eventListener = null;
  const bridge = createApplicationBridge({
    invoke: async (command, args) => {
      invocations.push({ command, args });
      return { acknowledged: true };
    },
    listen: async (eventName, listener) => {
      subscribedEvent = eventName;
      eventListener = listener;
      return () => {};
    },
  });

  await bridge.newSession({ workspace: "C:\\workspace" });
  await bridge.sendPrompt({ sessionId: "session-1", text: "hello" });
  let received = null;
  await bridge.onEvent((event) => {
    received = event;
  });
  eventListener({
    payload: {
      generation: 1,
      sequence: 1,
      event: { type: "runtime_state_changed", state: "ready" },
    },
  });

  assert.deepEqual(invocations, [
    {
      command: "session_new",
      args: { request: { workspace: "C:\\workspace" } },
    },
    {
      command: "prompt_send",
      args: { request: { sessionId: "session-1", text: "hello" } },
    },
  ]);
  assert.equal(subscribedEvent, "grok-application-event");
  assert.equal(received.event.state, "ready");
});
