import type { ApplicationTransport } from "./bridge.ts";
import type { ApplicationEvent, ApplicationEventEnvelope, Session } from "./contract.ts";

export const FIXTURE_WORKSPACE = "/Users/fixture/workspace";
export const FIXTURE_SESSION_ID = "session-fixture";

const capabilities = {
  protocolVersion: 1,
  agent: { product: "grok_build" as const, version: "fixture" },
  authenticationMethods: ["cached_token" as const],
  sessions: {
    create: true,
    prompt: true,
    cancel: true,
    list: true,
    load: true,
    resume: true,
    close: true,
  },
  models: fixtureModels("fixture-fast"),
  truncated: false,
};

const fixtureSession = (): Session => ({
  sessionId: FIXTURE_SESSION_ID,
  models: fixtureModels("fixture-fast"),
  legacyConfigOptions: [],
  controls: {
    modes: {
      currentModeId: "default",
      availableModes: [
        { id: "default", name: "Default", description: "Ordinary turns" },
        { id: "plan", name: "Plan", description: "Write a plan first" },
      ],
      truncated: false,
    },
    configOptions: [
      {
        id: "verbose",
        name: "Verbose",
        description: "Show more tool detail",
        kind: { type: "boolean", currentValue: false },
      },
    ],
    truncated: false,
  },
  truncated: false,
});

export function createConversationFixtureTransport(): ApplicationTransport {
  const listeners = new Set<(event: { payload: ApplicationEventEnvelope }) => void>();
  let sequence = 0;
  const generation = 1;

  const emit = (event: ApplicationEvent) => {
    sequence += 1;
    const envelope = { generation, sequence, event };
    for (const listener of listeners) {
      listener({ payload: envelope });
    }
  };

  return {
    invoke: async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
      return asCommandResult(
        dispatchFixtureCommand({
          command,
          request: isRecord(args?.request) ? args.request : {},
          emit,
          generation,
          sequence,
        }),
      );
    },
    listen: async <T>(_eventName: string, listener: (event: { payload: T }) => void) => {
      const wrapped = (event: { payload: ApplicationEventEnvelope }) => {
        listener({ payload: event.payload as T });
      };
      listeners.add(wrapped);
      return () => {
        listeners.delete(wrapped);
      };
    },
  };
}

function dispatchFixtureCommand(input: {
  command: string;
  request: Record<string, unknown>;
  emit: (event: ApplicationEvent) => void;
  generation: number;
  sequence: number;
}): unknown {
  const { command, request, emit, generation, sequence } = input;
  switch (command) {
    case "setup_status":
      return {
        runtimeAvailable: true,
        executableState: "available",
        executableSource: "user_install",
        failure: null,
      };
    case "runtime_snapshot":
    case "runtime_start":
      return {
        generation,
        lastSequence: sequence,
        state: "ready",
        capabilities,
      };
    case "workspace_recent_list":
      return {
        workspaces: [{ path: FIXTURE_WORKSPACE, available: true }],
      };
    case "workspace_validate":
    case "workspace_pick":
      return { path: FIXTURE_WORKSPACE };
    case "session_list":
      return {
        sessions: [
          {
            sessionId: FIXTURE_SESSION_ID,
            workspace: FIXTURE_WORKSPACE,
            title: "Fixture session",
            updatedAt: "2030-01-01T00:00:00Z",
          },
        ],
        nextCursor: null,
        truncated: false,
      };
    case "session_new":
    case "session_load":
    case "session_resume": {
      const session = fixtureSession();
      emit({ type: "session_activated", sessionId: session.sessionId });
      emit({
        type: "session_state_changed",
        sessionId: session.sessionId,
        state: "ready",
      });
      emit({
        type: "available_commands_changed",
        sessionId: session.sessionId,
        commands: [
          { name: "review", description: "Review the current change", acceptsInput: false },
        ],
        truncated: false,
      });
      emit({
        type: "session_mode_changed",
        sessionId: session.sessionId,
        currentModeId: "default",
      });
      emit({
        type: "session_config_options_changed",
        sessionId: session.sessionId,
        configOptions: session.controls.configOptions,
        truncated: false,
      });
      return session;
    }
    case "prompt_send": {
      const text = String(request.text ?? "");
      emit({
        type: "session_state_changed",
        sessionId: FIXTURE_SESSION_ID,
        state: "working",
      });
      emit({
        type: "user_message_chunk_received",
        sessionId: FIXTURE_SESSION_ID,
        messageId: "user-turn",
        text,
        truncated: false,
      });
      emit({
        type: "thought_chunk_received",
        sessionId: FIXTURE_SESSION_ID,
        thoughtId: "thought-1",
        text: "Checking the workspace before answering.",
        truncated: false,
      });
      emit({
        type: "tool_call_changed",
        sessionId: FIXTURE_SESSION_ID,
        callId: "term-1",
        title: "List files",
        kind: "terminal_command",
        status: "running",
        detail: "$ ls\nsrc\nREADME.md\n",
      });
      emit({
        type: "tool_call_changed",
        sessionId: FIXTURE_SESSION_ID,
        callId: "term-1",
        title: "List files",
        kind: "terminal_command",
        status: "completed",
        detail: "$ ls\nsrc\nREADME.md\n",
      });
      emit({
        type: "message_chunk_received",
        sessionId: FIXTURE_SESSION_ID,
        messageId: "assistant-1",
        text: "I listed the workspace. See the [docs](https://example.com/docs) for next steps.\n\n- One\n- Two\n",
        truncated: false,
      });
      emit({
        type: "usage_changed",
        sessionId: FIXTURE_SESSION_ID,
        usage: {
          inputTokens: 12,
          outputTokens: 9,
          cachedInputTokens: 2,
          totalTokens: 23,
          contextWindowTokens: 128000,
        },
      });
      emit({
        type: "plan_changed",
        sessionId: FIXTURE_SESSION_ID,
        entries: [
          { id: "step-1", title: "Inspect", description: null, status: "completed" },
          { id: "step-2", title: "Answer", description: null, status: "in_progress" },
        ],
        truncated: false,
      });
      emit({
        type: "session_state_changed",
        sessionId: FIXTURE_SESSION_ID,
        state: "ready",
      });
      return { stopReason: "end_turn" };
    }
    case "prompt_cancel":
      emit({
        type: "session_state_changed",
        sessionId: FIXTURE_SESSION_ID,
        state: "ready",
      });
      return { acknowledged: true };
    case "session_set_mode":
      emit({
        type: "session_mode_changed",
        sessionId: FIXTURE_SESSION_ID,
        currentModeId: String(request.modeId ?? "default"),
      });
      return { acknowledged: true };
    case "session_set_model":
    case "session_set_config":
    case "open_external_url":
    case "session_close":
      return { acknowledged: true };
    case "workspace_recent_remove":
      return { workspaces: [] };
    default:
      throw {
        code: "invalid_request",
        diagnostic: `Unsupported fixture command: ${command}`,
        recoverable: false,
      };
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function asCommandResult<T>(value: unknown): T {
  return value as T;
}

function fixtureModels(currentModelId: string) {
  return {
    currentModelId,
    availableModels: [
      {
        modelId: "fixture-fast",
        name: "Fixture Fast",
        description: "Short fixture answers",
        agentType: null,
        reasoningEffort: "low",
        reasoningEfforts: [
          {
            id: "low",
            label: "Low",
            description: null,
            value: "low",
            isDefault: true,
          },
          {
            id: "high",
            label: "High",
            description: null,
            value: "high",
            isDefault: false,
          },
        ],
        supportsReasoningEffort: true,
        totalContextTokens: 128000,
        truncated: false,
      },
      {
        modelId: "fixture-deep",
        name: "Fixture Deep",
        description: "Longer fixture answers",
        agentType: null,
        reasoningEffort: "high",
        reasoningEfforts: [
          {
            id: "high",
            label: "High",
            description: null,
            value: "high",
            isDefault: true,
          },
        ],
        supportsReasoningEffort: true,
        totalContextTokens: 256000,
        truncated: false,
      },
    ],
    truncated: false,
  };
}
