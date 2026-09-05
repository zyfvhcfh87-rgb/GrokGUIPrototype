import type {
  Acknowledgement,
  ApplicationError,
  ApplicationEventEnvelope,
  ListSessionsRequest,
  NewSessionRequest,
  RuntimeCapabilities,
  RuntimeSessionCapabilities,
  RuntimeState,
  Session,
  SessionPage,
  SessionRequest,
  SessionWorkspaceRequest,
  Workspace,
} from "./contract.ts";
import {
  isRuntimeUsable,
  sessionBelongsToWorkspace,
  type SessionListKind,
  type SessionRow,
  type SessionRowStatus,
} from "./sessions.ts";

type Unlisten = () => void;

export type SessionControllerBridge = {
  onEvent(listener: (event: ApplicationEventEnvelope) => void): Promise<Unlisten>;
  listSessions(request: ListSessionsRequest): Promise<SessionPage>;
  newSession(request: NewSessionRequest): Promise<Session>;
  loadSession(request: SessionWorkspaceRequest): Promise<Session>;
  resumeSession(request: SessionWorkspaceRequest): Promise<Session>;
  closeSession(request: SessionRequest): Promise<Acknowledgement>;
};

export type SessionControllerState = {
  workspace: string | null;
  runtimeState: RuntimeState;
  capabilities: RuntimeSessionCapabilities | null;
  listKind: SessionListKind;
  sessions: SessionRow[];
  selectedSessionId: string | null;
  selectedSession: Session | null;
  nextCursor: string | null;
  truncated: boolean;
  failure: ApplicationError | null;
  creating: boolean;
  opening: boolean;
  closing: boolean;
};

const INITIAL_STATE: SessionControllerState = {
  workspace: null,
  runtimeState: "disconnected",
  capabilities: null,
  listKind: "idle",
  sessions: [],
  selectedSessionId: null,
  selectedSession: null,
  nextCursor: null,
  truncated: false,
  failure: null,
  creating: false,
  opening: false,
  closing: false,
};

export function createSessionController(bridge: SessionControllerBridge) {
  let state = INITIAL_STATE;
  let unlisten: Unlisten | null = null;
  let lifecycle = 0;
  let listToken = 0;
  let generation = 0;
  const closedIds = new Set<string>();
  const failedIds = new Set<string>();
  const listeners = new Set<(state: SessionControllerState) => void>();

  const publish = (patch: Partial<SessionControllerState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) {
      listener(state);
    }
  };

  const capabilities = (value: RuntimeCapabilities | null) => value?.sessions ?? null;

  const canList = (runtimeState = state.runtimeState, sessions = state.capabilities) =>
    state.workspace !== null && isRuntimeUsable(runtimeState) && (sessions?.list ?? false);

  const rowStatus = (sessionId: string, selectedSessionId: string | null): SessionRowStatus => {
    if (failedIds.has(sessionId)) {
      return "failed";
    }
    if (sessionId === selectedSessionId) {
      return "selected";
    }
    if (closedIds.has(sessionId)) {
      return "closed";
    }
    return "listed";
  };

  const applyRows = (
    sessions: SessionRow[],
    extras: Partial<SessionControllerState> = {},
  ): void => {
    const selectedSessionId =
      extras.selectedSessionId !== undefined
        ? extras.selectedSessionId
        : state.selectedSessionId !== null &&
            sessions.some((session) => session.sessionId === state.selectedSessionId)
          ? state.selectedSessionId
          : null;
    const selectedSession =
      extras.selectedSession !== undefined
        ? extras.selectedSession
        : selectedSessionId === state.selectedSessionId
          ? state.selectedSession
          : null;
    publish({
      ...extras,
      selectedSessionId,
      selectedSession,
      sessions: sessions.map((session) => ({
        ...session,
        status: rowStatus(session.sessionId, selectedSessionId),
      })),
    });
  };

  const refreshList = async (token: number) => {
    if (!canList()) {
      publish({
        listKind: state.workspace === null ? "idle" : "unavailable",
        sessions: [],
      });
      return;
    }
    const workspace = state.workspace;
    if (workspace === null) {
      return;
    }
    publish({ listKind: "loading", failure: null });
    try {
      const page = await bridge.listSessions({ workspace, cursor: null });
      if (token !== listToken || state.workspace !== workspace) {
        return;
      }
      const scoped = page.sessions
        .filter((session) => sessionBelongsToWorkspace(session, workspace))
        .map((session) => ({ ...session, status: "listed" as const }));
      applyRows(scoped, {
        listKind: scoped.length === 0 ? "empty" : "ready",
        nextCursor: page.nextCursor,
        truncated: page.truncated,
        failure: null,
      });
    } catch (error) {
      if (token !== listToken || state.workspace !== workspace) {
        return;
      }
      publish({
        listKind: "failed",
        failure: applicationError(error, "Sessions for this workspace could not be loaded."),
      });
    }
  };

  const handleEvent = (envelope: ApplicationEventEnvelope) => {
    if (envelope.generation < generation) {
      return;
    }
    if (envelope.generation > generation) {
      generation = envelope.generation;
      closedIds.clear();
      failedIds.clear();
      if (state.workspace !== null) {
        publish({
          selectedSessionId: null,
          selectedSession: null,
          listKind: "stale",
        });
        void refreshList(++listToken);
      }
      return;
    }

    const event = envelope.event;
    if (event.type === "runtime_extension_invalidated" && event.area === "sessions") {
      if (state.workspace !== null) {
        publish({ listKind: "stale" });
        void refreshList(++listToken);
      }
      return;
    }
    if (event.type === "session_info_changed" && state.workspace !== null) {
      applyRows(
        state.sessions.map((session) =>
          session.sessionId === event.sessionId
            ? {
                ...session,
                title:
                  event.title.state === "cleared"
                    ? null
                    : event.title.state === "value"
                      ? event.title.value
                      : session.title,
                updatedAt:
                  event.updatedAt.state === "cleared"
                    ? null
                    : event.updatedAt.state === "value"
                      ? event.updatedAt.value
                      : session.updatedAt,
              }
            : session,
        ),
      );
      return;
    }
    if (event.type === "session_state_changed") {
      if (event.state === "closed") {
        closedIds.add(event.sessionId);
        failedIds.delete(event.sessionId);
        applyRows(state.sessions, {
          selectedSessionId:
            state.selectedSessionId === event.sessionId ? null : state.selectedSessionId,
          selectedSession:
            state.selectedSessionId === event.sessionId ? null : state.selectedSession,
        });
      }
      if (event.state === "failed") {
        failedIds.add(event.sessionId);
        applyRows(state.sessions);
      }
      if (event.state === "ready") {
        failedIds.delete(event.sessionId);
        closedIds.delete(event.sessionId);
        applyRows(state.sessions);
      }
    }
  };

  const requireWorkspace = (): string => {
    if (state.workspace === null) {
      throw applicationError(
        {
          code: "invalid_workspace",
          diagnostic: "Choose a workspace before opening a session.",
          recoverable: true,
        },
        "Choose a workspace before opening a session.",
        "invalid_workspace",
      );
    }
    return state.workspace;
  };

  const openWith = async (
    sessionId: string,
    method: "resume" | "load",
  ): Promise<Session> => {
    const workspace = requireWorkspace();
    const listed = state.sessions.find((session) => session.sessionId === sessionId);
    if (listed === undefined || !sessionBelongsToWorkspace(listed, workspace)) {
      const failure = applicationError(
        {
          code: "invalid_workspace",
          diagnostic: "That session belongs to another workspace.",
          recoverable: true,
        },
        "That session belongs to another workspace.",
        "invalid_workspace",
      );
      publish({ failure });
      throw failure;
    }
    publish({ opening: true, failure: null });
    try {
      const request = { sessionId, workspace };
      const session =
        method === "resume"
          ? await bridge.resumeSession(request)
          : await bridge.loadSession(request);
      if (state.workspace !== workspace) {
        return session;
      }
      closedIds.delete(sessionId);
      failedIds.delete(sessionId);
      applyRows(state.sessions, {
        selectedSessionId: session.sessionId,
        selectedSession: session,
        opening: false,
      });
      return session;
    } catch (error) {
      failedIds.add(sessionId);
      const failure = applicationError(
        error,
        method === "resume"
          ? "That session could not be resumed."
          : "That session could not be loaded.",
      );
      applyRows(state.sessions, { opening: false, failure });
      throw failure;
    }
  };

  return {
    getState: () => state,
    subscribe: (listener: (state: SessionControllerState) => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    initialize: async () => {
      if (unlisten !== null) {
        return;
      }
      const currentLifecycle = ++lifecycle;
      const nextUnlisten = await bridge.onEvent(handleEvent);
      if (currentLifecycle !== lifecycle) {
        nextUnlisten();
        return;
      }
      unlisten = nextUnlisten;
    },
    setRuntime: (runtimeState: RuntimeState, runtimeCapabilities: RuntimeCapabilities | null) => {
      const sessions = capabilities(runtimeCapabilities);
      const shouldLoad = state.workspace !== null && !canList() && canList(runtimeState, sessions);
      publish({ runtimeState, capabilities: sessions });
      if (shouldLoad) {
        void refreshList(++listToken);
      } else if (state.workspace !== null && !isRuntimeUsable(runtimeState)) {
        publish({ listKind: "unavailable" });
      }
    },
    setWorkspace: async (workspace: Workspace | null) => {
      const path = workspace?.path ?? null;
      if (path === state.workspace) {
        return;
      }
      const token = ++listToken;
      const nextCanList =
        path !== null &&
        isRuntimeUsable(state.runtimeState) &&
        (state.capabilities?.list ?? false);
      closedIds.clear();
      failedIds.clear();
      publish({
        workspace: path,
        sessions: [],
        selectedSessionId: null,
        selectedSession: null,
        nextCursor: null,
        truncated: false,
        failure: null,
        creating: false,
        opening: false,
        closing: false,
        listKind: path === null ? "idle" : nextCanList ? "loading" : "unavailable",
      });
      if (nextCanList) {
        await refreshList(token);
      }
    },
    refresh: async () => {
      await refreshList(++listToken);
    },
    createSession: async () => {
      const workspace = requireWorkspace();
      publish({ creating: true, failure: null });
      try {
        const session = await bridge.newSession({ workspace });
        if (state.workspace !== workspace) {
          return session;
        }
        closedIds.delete(session.sessionId);
        failedIds.delete(session.sessionId);
        applyRows(
          [
            {
              sessionId: session.sessionId,
              workspace,
              title: null,
              updatedAt: null,
              status: "selected",
            },
            ...state.sessions.filter((item) => item.sessionId !== session.sessionId),
          ],
          {
            selectedSessionId: session.sessionId,
            selectedSession: session,
            creating: false,
            listKind: "ready",
          },
        );
        void refreshList(++listToken);
        return session;
      } catch (error) {
        const failure = applicationError(error, "A new session could not be created.");
        publish({ creating: false, failure, listKind: state.sessions.length === 0 ? "failed" : state.listKind });
        throw failure;
      }
    },
    openSession: async (sessionId: string) => {
      if (state.capabilities?.resume) {
        return openWith(sessionId, "resume");
      }
      if (state.capabilities?.load) {
        return openWith(sessionId, "load");
      }
      const failure = applicationError(
        {
          code: "capability_unavailable",
          diagnostic: "This runtime cannot load or resume sessions.",
          recoverable: false,
        },
        "This runtime cannot load or resume sessions.",
        "capability_unavailable",
      );
      publish({ failure });
      throw failure;
    },
    loadSession: (sessionId: string) => openWith(sessionId, "load"),
    resumeSession: (sessionId: string) => openWith(sessionId, "resume"),
    closeSession: async (sessionId: string) => {
      requireWorkspace();
      publish({ closing: true, failure: null });
      try {
        await bridge.closeSession({ sessionId });
        closedIds.add(sessionId);
        failedIds.delete(sessionId);
        applyRows(state.sessions, {
          selectedSessionId: state.selectedSessionId === sessionId ? null : state.selectedSessionId,
          selectedSession: state.selectedSessionId === sessionId ? null : state.selectedSession,
          closing: false,
        });
        void refreshList(++listToken);
      } catch (error) {
        const failure = applicationError(error, "That session could not be closed.");
        publish({ closing: false, failure });
        throw failure;
      }
    },
    dispose: () => {
      lifecycle += 1;
      listToken += 1;
      unlisten?.();
      unlisten = null;
      listeners.clear();
    },
  } as const;
}

function applicationError(
  value: unknown,
  fallbackDiagnostic: string,
  fallbackCode: ApplicationError["code"] = "protocol_request_failed",
): ApplicationError {
  if (isApplicationError(value)) {
    return value;
  }
  return {
    code: fallbackCode,
    diagnostic: fallbackDiagnostic,
    recoverable: true,
  };
}

function isApplicationError(value: unknown): value is ApplicationError {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Partial<ApplicationError>;
  return (
    typeof candidate.code === "string" &&
    typeof candidate.diagnostic === "string" &&
    typeof candidate.recoverable === "boolean"
  );
}
