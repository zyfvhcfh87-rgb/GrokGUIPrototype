import type {
  ApplicationEvent,
  ApplicationEventEnvelope,
  AvailableCommand,
  ConfigOption,
  ElicitationControl,
  PermissionScope,
  PlanEntry,
  RuntimeExtensionArea,
  RuntimeState,
  SessionState,
  ToolCallKind,
  ActivityStatus,
  Usage,
  PermissionDecision,
} from "./contract.ts";

const MAX_PENDING_EVENTS = 128;

export type StreamChunk = {
  id: string;
  text: string;
  truncated: boolean;
};

export type ToolCallState = {
  id: string;
  title: string;
  kind: ToolCallKind;
  status: ActivityStatus;
  detail: string | null;
};

export type PermissionRequestState = {
  id: string;
  title: string;
  consequence: string | null;
  scope: PermissionScope;
  availableDecisions: PermissionDecision[];
};

export type ElicitationRequestState = {
  id: string;
  prompt: string;
  control: ElicitationControl;
};

export type SessionViewState = {
  state: SessionState;
  title: string | null;
  updatedAt: string | null;
  messages: Record<string, StreamChunk>;
  thoughts: Record<string, StreamChunk>;
  toolCalls: Record<string, ToolCallState>;
  permissions: Record<string, PermissionRequestState>;
  elicitations: Record<string, ElicitationRequestState>;
  plan: PlanEntry[];
  usage: Usage | null;
  availableCommands: AvailableCommand[];
  currentModeId: string | null;
  configOptions: ConfigOption[];
  metadataRevision: number;
  invalidatedAreas: RuntimeExtensionArea[];
};

export type ApplicationState = {
  generation: number;
  lastSequence: number;
  pendingEventCount: number;
  needsResync: boolean;
  runtime: {
    state: RuntimeState;
    failure: { diagnostic: string; recoverable: boolean } | null;
    invalidatedAreas: RuntimeExtensionArea[];
    elicitations: Record<string, ElicitationRequestState>;
  };
  sessions: Record<string, SessionViewState>;
  observedExtensions: { unknown: number; invalid: number };
  pendingEvents: Record<number, ApplicationEventEnvelope>;
};

export function initialApplicationState(generation = 0): ApplicationState {
  return {
    generation,
    lastSequence: 0,
    pendingEventCount: 0,
    needsResync: false,
    runtime: {
      state: "disconnected",
      failure: null,
      invalidatedAreas: [],
      elicitations: {},
    },
    sessions: {},
    observedExtensions: { unknown: 0, invalid: 0 },
    pendingEvents: {},
  };
}

export function createApplicationStore(initial = initialApplicationState()) {
  let state = initial;
  const listeners = new Set<(state: ApplicationState) => void>();

  return {
    getState: () => state,
    dispatch: (event: ApplicationEventEnvelope) => {
      const next = reduceApplicationEvent(state, event);
      if (next === state) {
        return;
      }
      state = next;
      for (const listener of listeners) {
        listener(state);
      }
    },
    subscribe: (listener: (state: ApplicationState) => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  } as const;
}

export function reduceApplicationEvent(
  current: ApplicationState,
  envelope: ApplicationEventEnvelope,
): ApplicationState {
  if (!isValidPosition(envelope) || envelope.generation < current.generation) {
    return current;
  }

  let state = current;
  if (envelope.generation > current.generation) {
    state = initialApplicationState(envelope.generation);
  }

  if (envelope.sequence <= state.lastSequence) {
    return state;
  }

  const gap = envelope.sequence - state.lastSequence;
  if (gap > MAX_PENDING_EVENTS || state.pendingEventCount >= MAX_PENDING_EVENTS) {
    return {
      ...state,
      needsResync: true,
      pendingEvents: {},
      pendingEventCount: 0,
    };
  }

  if (state.pendingEvents[envelope.sequence] !== undefined) {
    return state;
  }

  const pendingEvents = {
    ...state.pendingEvents,
    [envelope.sequence]: envelope,
  };
  state = {
    ...state,
    pendingEvents,
    pendingEventCount: state.pendingEventCount + 1,
  };

  while (state.pendingEvents[state.lastSequence + 1] !== undefined) {
    const nextSequence = state.lastSequence + 1;
    const nextEnvelope = state.pendingEvents[nextSequence];
    if (nextEnvelope === undefined) {
      break;
    }
    const remaining = { ...state.pendingEvents };
    delete remaining[nextSequence];
    state = applyEvent(
      {
        ...state,
        lastSequence: nextSequence,
        pendingEvents: remaining,
        pendingEventCount: state.pendingEventCount - 1,
      },
      nextEnvelope.event,
    );
  }

  return state;
}

function isValidPosition(envelope: ApplicationEventEnvelope): boolean {
  return (
    Number.isSafeInteger(envelope.generation) &&
    envelope.generation >= 0 &&
    Number.isSafeInteger(envelope.sequence) &&
    envelope.sequence > 0
  );
}

function applyEvent(state: ApplicationState, event: ApplicationEvent): ApplicationState {
  if (event.type === "runtime_state_changed") {
    const safeState =
      event.state === "disconnected" || event.state === "failed"
        ? clearInteractions(state, null)
        : state;
    return {
      ...safeState,
      runtime: {
        ...safeState.runtime,
        state: event.state,
        failure: event.state === "failed" ? safeState.runtime.failure : null,
      },
    };
  }

  if (event.type === "runtime_failed") {
    const safeState = clearInteractions(state, null);
    return {
      ...safeState,
      runtime: {
        ...safeState.runtime,
        state: "failed",
        failure: {
          diagnostic: event.diagnostic,
          recoverable: event.recoverable,
        },
      },
    };
  }

  if (event.type === "extension_observed") {
    return {
      ...state,
      observedExtensions: {
        ...state.observedExtensions,
        [event.classification]: state.observedExtensions[event.classification] + 1,
      },
    };
  }

  if (event.type === "runtime_extension_invalidated" && event.sessionId === null) {
    return {
      ...state,
      runtime: {
        ...state.runtime,
        invalidatedAreas: appendUnique(state.runtime.invalidatedAreas, event.area),
      },
    };
  }

  if (event.type === "elicitation_requested" && event.sessionId === null) {
    if (state.runtime.state === "failed" || state.runtime.state === "disconnected") {
      return state;
    }
    return {
      ...state,
      runtime: {
        ...state.runtime,
        elicitations: {
          ...state.runtime.elicitations,
          [event.interactionId]: {
            id: event.interactionId,
            prompt: event.prompt,
            control: event.control,
          },
        },
      },
    };
  }

  if (event.type === "interaction_resolved") {
    return resolveInteraction(state, event.interactionId, event.kind);
  }

  if (event.type === "interactions_cleared") {
    return clearInteractions(state, event.sessionId);
  }

  const sessionId = sessionIdForEvent(event);
  if (sessionId === null) {
    return state;
  }

  if (
    (event.type === "permission_requested" || event.type === "elicitation_requested") &&
    (state.runtime.state === "failed" || state.runtime.state === "disconnected")
  ) {
    return state;
  }

  const existing = state.sessions[sessionId];
  if (event.type === "session_activated") {
    return {
      ...state,
      sessions: { ...state.sessions, [sessionId]: initialSessionState() },
    };
  }
  if (existing?.state === "closed") {
    return state;
  }
  if (
    existing !== undefined &&
    (event.type === "permission_requested" || event.type === "elicitation_requested") &&
    (existing.state === "cancelling" ||
      existing.state === "completed" ||
      existing.state === "failed")
  ) {
    return state;
  }
  const session = existing ?? initialSessionState();
  const nextSession = reduceSessionEvent(session, event);
  return {
    ...state,
    sessions: { ...state.sessions, [sessionId]: nextSession },
  };
}

function initialSessionState(): SessionViewState {
  return {
    state: "creating",
    title: null,
    updatedAt: null,
    messages: {},
    thoughts: {},
    toolCalls: {},
    permissions: {},
    elicitations: {},
    plan: [],
    usage: null,
    availableCommands: [],
    currentModeId: null,
    configOptions: [],
    metadataRevision: 0,
    invalidatedAreas: [],
  };
}

function reduceSessionEvent(
  session: SessionViewState,
  event: ApplicationEvent,
): SessionViewState {
  switch (event.type) {
    case "session_state_changed":
      return event.state === "cancelling" ||
        event.state === "completed" ||
        event.state === "closed" ||
        event.state === "failed"
        ? { ...session, state: event.state, permissions: {}, elicitations: {} }
        : { ...session, state: event.state };
    case "user_message_chunk_received":
      return appendChunk(session, "messages", `user:${event.messageId ?? "stream"}`, event);
    case "message_chunk_received":
      return appendChunk(
        session,
        "messages",
        `assistant:${event.messageId ?? "stream"}`,
        event,
      );
    case "thought_chunk_received":
      return appendChunk(session, "thoughts", event.thoughtId ?? "stream", event);
    case "tool_call_changed":
      return {
        ...session,
        toolCalls: {
          ...session.toolCalls,
          [event.callId]: {
            id: event.callId,
            title: event.title,
            kind: event.kind,
            status: event.status,
            detail: event.detail,
          },
        },
      };
    case "permission_requested":
      return {
        ...session,
        permissions: {
          ...session.permissions,
          [event.interactionId]: {
            id: event.interactionId,
            title: event.title,
            consequence: event.consequence,
            scope: event.scope,
            availableDecisions: event.availableDecisions,
          },
        },
      };
    case "elicitation_requested":
      return {
        ...session,
        elicitations: {
          ...session.elicitations,
          [event.interactionId]: {
            id: event.interactionId,
            prompt: event.prompt,
            control: event.control,
          },
        },
      };
    case "plan_changed":
      return { ...session, plan: event.entries };
    case "usage_changed":
      return { ...session, usage: event.usage };
    case "session_metadata_changed":
      return { ...session, metadataRevision: session.metadataRevision + 1 };
    case "available_commands_changed":
      return { ...session, availableCommands: event.commands };
    case "session_mode_changed":
      return { ...session, currentModeId: event.currentModeId };
    case "session_config_options_changed":
      return { ...session, configOptions: event.configOptions };
    case "session_info_changed":
      return {
        ...session,
        title: applyOptionalUpdate(session.title, event.title),
        updatedAt: applyOptionalUpdate(session.updatedAt, event.updatedAt),
      };
    case "runtime_extension_invalidated":
      return {
        ...session,
        invalidatedAreas: appendUnique(session.invalidatedAreas, event.area),
      };
    default:
      return session;
  }
}

function resolveInteraction(
  state: ApplicationState,
  interactionId: string,
  kind: "permission" | "elicitation",
): ApplicationState {
  let changed = false;
  const sessions = { ...state.sessions };
  for (const [sessionId, session] of Object.entries(state.sessions)) {
    if (kind === "permission" && session.permissions[interactionId] !== undefined) {
      changed = true;
      const permissions = { ...session.permissions };
      delete permissions[interactionId];
      sessions[sessionId] = { ...session, permissions };
    }
    if (kind === "elicitation" && session.elicitations[interactionId] !== undefined) {
      changed = true;
      const elicitations = { ...session.elicitations };
      delete elicitations[interactionId];
      sessions[sessionId] = { ...session, elicitations };
    }
  }

  let runtime = state.runtime;
  if (kind === "elicitation" && runtime.elicitations[interactionId] !== undefined) {
    changed = true;
    const elicitations = { ...runtime.elicitations };
    delete elicitations[interactionId];
    runtime = { ...runtime, elicitations };
  }

  return changed ? { ...state, runtime, sessions } : state;
}

function clearInteractions(
  state: ApplicationState,
  sessionId: string | null,
): ApplicationState {
  if (sessionId !== null) {
    const session = state.sessions[sessionId];
    if (session === undefined) {
      return state;
    }
    return {
      ...state,
      sessions: {
        ...state.sessions,
        [sessionId]: { ...session, permissions: {}, elicitations: {} },
      },
    };
  }

  return {
    ...state,
    runtime: { ...state.runtime, elicitations: {} },
    sessions: Object.fromEntries(
      Object.entries(state.sessions).map(([id, session]) => [
        id,
        { ...session, permissions: {}, elicitations: {} },
      ]),
    ),
  };
}

function appendChunk(
  session: SessionViewState,
  collection: "messages" | "thoughts",
  id: string,
  event: { text: string; truncated: boolean },
): SessionViewState {
  const chunks = session[collection];
  const previous = chunks[id];
  return {
    ...session,
    [collection]: {
      ...chunks,
      [id]: {
        id,
        text: `${previous?.text ?? ""}${event.text}`,
        truncated: (previous?.truncated ?? false) || event.truncated,
      },
    },
  };
}

function sessionIdForEvent(event: ApplicationEvent): string | null {
  if ("sessionId" in event) {
    return event.sessionId;
  }
  return null;
}

function applyOptionalUpdate(
  current: string | null,
  update: { state: "not_reported" | "cleared" | "value"; value?: string },
): string | null {
  if (update.state === "not_reported") {
    return current;
  }
  return update.state === "cleared" ? null : (update.value ?? null);
}

function appendUnique<T>(values: T[], value: T): T[] {
  return values.includes(value) ? values : [...values, value];
}
