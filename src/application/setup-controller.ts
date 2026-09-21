import type {
  ApplicationError,
  ApplicationEvent,
  ApplicationEventEnvelope,
  RecentWorkspace,
  RecentWorkspaceList,
  RuntimeSnapshot,
  RuntimeState,
  SetupStatus,
  Workspace,
  WorkspaceRequest,
} from "./contract.ts";
import { createApplicationStore, type ApplicationStore } from "./state.ts";

type Unlisten = () => void;

export type SetupControllerBridge = {
  onEvent(listener: (event: ApplicationEventEnvelope) => void): Promise<Unlisten>;
  setupStatus(): Promise<SetupStatus>;
  listRecentWorkspaces(): Promise<RecentWorkspaceList>;
  runtimeSnapshot(): Promise<RuntimeSnapshot>;
  startRuntime?(): Promise<RuntimeSnapshot>;
  restartRuntime?(): Promise<RuntimeSnapshot>;
  pickWorkspace?(): Promise<Workspace | null>;
  validateWorkspace?(request: WorkspaceRequest): Promise<Workspace>;
  removeRecentWorkspace?(request: WorkspaceRequest): Promise<RecentWorkspaceList>;
};

export type SetupControllerState = {
  setup: SetupStatus | null;
  runtimeState: RuntimeState;
  capabilities: RuntimeSnapshot["capabilities"];
  failure: ApplicationError | null;
  recentWorkspaces: RecentWorkspace[];
  selectedWorkspace: Workspace | null;
  workspaceFailure: ApplicationError | null;
  initializing: boolean;
  choosingWorkspace: boolean;
};

const INITIAL_STATE: SetupControllerState = {
  setup: null,
  runtimeState: "disconnected",
  capabilities: null,
  failure: null,
  recentWorkspaces: [],
  selectedWorkspace: null,
  workspaceFailure: null,
  initializing: true,
  choosingWorkspace: false,
};

export function createSetupController(
  bridge: SetupControllerBridge,
  applicationStore?: ApplicationStore,
) {
  const ownsStore = applicationStore === undefined;
  const store = applicationStore ?? createApplicationStore();
  let state = INITIAL_STATE;
  let unlisten: Unlisten | null = null;
  let unsubscribeStore: Unlisten | null = null;
  let lifecycle = 0;
  let restoredWorkspace = false;
  const listeners = new Set<(state: SetupControllerState) => void>();

  const publish = (patch: Partial<SetupControllerState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) {
      listener(state);
    }
  };

  const syncRuntime = () => {
    const runtime = store.getState().runtime;
    const failure =
      runtime.failure === null
        ? runtime.state === "failed"
          ? state.failure
          : null
        : {
            code: "connection_failed" as const,
            diagnostic: runtime.failure.diagnostic,
            recoverable: runtime.failure.recoverable,
          };
    if (runtime.state === state.runtimeState && failureSame(failure, state.failure)) {
      return;
    }
    publish({
      runtimeState: runtime.state,
      failure,
    });
  };

  const handleEvent = (envelope: ApplicationEventEnvelope) => {
    if (!isRuntimeEvent(envelope.event)) {
      if (ownsStore) {
        store.observeSequence(envelope);
      }
      return;
    }
    store.dispatch(envelope);
  };

  const acceptSnapshot = (snapshot: RuntimeSnapshot) => {
    store.adoptAuthoritativeSnapshot({
      generation: snapshot.generation,
      lastSequence: snapshot.lastSequence,
      state: snapshot.state,
    });
    return store.getState().runtime.state;
  };

  const refreshRecent = async () => {
    try {
      const result = await bridge.listRecentWorkspaces();
      publish({ recentWorkspaces: result.workspaces, workspaceFailure: null });
    } catch (error) {
      publish({
        workspaceFailure: applicationError(
          error,
          "Recent workspaces could not be loaded.",
          "preferences_unavailable",
        ),
      });
    }
  };

  const restoreWorkspaceIfNeeded = async () => {
    if (restoredWorkspace || state.selectedWorkspace !== null) {
      restoredWorkspace = true;
      return;
    }
    if (state.runtimeState !== "ready" || bridge.validateWorkspace === undefined) {
      return;
    }
    const firstAvailable = state.recentWorkspaces.find((workspace) => workspace.available);
    restoredWorkspace = true;
    if (firstAvailable === undefined) {
      return;
    }
    try {
      const selected = await bridge.validateWorkspace({ path: firstAvailable.path });
      if (state.selectedWorkspace === null) {
        publish({ selectedWorkspace: selected, workspaceFailure: null });
      }
      await refreshRecent();
    } catch (error) {
      publish({
        workspaceFailure: applicationError(error, "That workspace is no longer available."),
      });
    }
  };

  const connectWith = async (method: "start" | "restart") => {
    const run =
      method === "restart" && bridge.restartRuntime !== undefined
        ? bridge.restartRuntime
        : bridge.startRuntime;
    if (run === undefined) {
      return;
    }
    try {
      const snapshot = await run();
      const runtimeState = acceptSnapshot(snapshot);
      publish({
        runtimeState,
        capabilities: snapshot.capabilities,
        failure: null,
      });
      if (runtimeState === "ready") {
        await restoreWorkspaceIfNeeded();
      }
    } catch (error) {
      publish({
        runtimeState: "failed",
        failure: applicationError(
          error,
          method === "restart"
            ? "Grok Build could not be recovered."
            : "Grok Build could not be started.",
        ),
      });
    }
  };

  const connect = () => connectWith("start");

  return {
    getState: () => state,
    subscribe: (listener: (state: SetupControllerState) => void) => {
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
      unsubscribeStore ??= store.subscribe(() => {
        syncRuntime();
      });
      try {
        const nextUnlisten = await bridge.onEvent(handleEvent);
        if (currentLifecycle !== lifecycle) {
          nextUnlisten();
          return;
        }
        unlisten = nextUnlisten;
        const [setupResult, snapshotResult, recent] = await Promise.all([
          bridge
            .setupStatus()
            .then((value) => ({ value, error: null }))
            .catch((error: unknown) => ({ value: null, error })),
          bridge
            .runtimeSnapshot()
            .then((value) => ({ value, error: null }))
            .catch((error: unknown) => ({ value: null, error })),
          bridge
            .listRecentWorkspaces()
            .then((value) => ({ value, error: null }))
            .catch((error: unknown) => ({ value: null, error })),
        ]);
        if (currentLifecycle !== lifecycle) {
          return;
        }
        if (setupResult.value === null) {
          throw setupResult.error;
        }
        const setup = setupResult.value;
        if (setup.runtimeAvailable && snapshotResult.value === null) {
          throw snapshotResult.error;
        }
        const snapshot = snapshotResult.value;
        publish({
          setup,
          runtimeState: snapshot === null ? "disconnected" : acceptSnapshot(snapshot),
          capabilities: snapshot?.capabilities ?? null,
          failure: setup.failure,
          recentWorkspaces: recent.value?.workspaces ?? [],
          workspaceFailure:
            recent.error === null
              ? null
              : applicationError(
                  recent.error,
                  "Recent workspaces could not be loaded.",
                  "preferences_unavailable",
                ),
          initializing: false,
        });
        if (setup.runtimeAvailable && snapshot?.state === "disconnected") {
          await connect();
        } else if (state.runtimeState === "ready") {
          await restoreWorkspaceIfNeeded();
        }
      } catch (error) {
        if (currentLifecycle !== lifecycle) {
          return;
        }
        publish({
          runtimeState: "failed",
          failure: applicationError(error, "The local setup check could not finish."),
          initializing: false,
        });
      }
    },
    retry: () => connectWith(state.runtimeState === "failed" ? "restart" : "start"),
    pickWorkspace: async () => {
      if (bridge.pickWorkspace === undefined) {
        return null;
      }
      publish({ choosingWorkspace: true, workspaceFailure: null });
      try {
        const selected = await bridge.pickWorkspace();
        if (selected !== null) {
          publish({ selectedWorkspace: selected });
          await refreshRecent();
        }
        return selected;
      } catch (error) {
        const failure = applicationError(error, "The workspace picker could not be opened.");
        publish({ workspaceFailure: failure });
        throw failure;
      } finally {
        publish({ choosingWorkspace: false });
      }
    },
    openRecent: async (path: string) => {
      if (bridge.validateWorkspace === undefined) {
        throw new Error("workspace validation is unavailable");
      }
      try {
        const selected = await bridge.validateWorkspace({ path });
        publish({ selectedWorkspace: selected, workspaceFailure: null });
        await refreshRecent();
        return selected;
      } catch (error) {
        const failure = applicationError(error, "That workspace is no longer available.");
        publish({ workspaceFailure: failure });
        throw failure;
      }
    },
    removeRecent: async (path: string) => {
      if (bridge.removeRecentWorkspace === undefined) {
        return;
      }
      try {
        const result = await bridge.removeRecentWorkspace({ path });
        publish({ recentWorkspaces: result.workspaces, workspaceFailure: null });
      } catch (error) {
        const failure = applicationError(error, "That recent workspace could not be removed.");
        publish({ workspaceFailure: failure });
        throw failure;
      }
    },
    dispose: () => {
      lifecycle += 1;
      unlisten?.();
      unlisten = null;
      unsubscribeStore?.();
      unsubscribeStore = null;
      listeners.clear();
    },
  } as const;
}

function isRuntimeEvent(event: ApplicationEvent): boolean {
  switch (event.type) {
    case "runtime_state_changed":
    case "runtime_failed":
    case "extension_observed":
    case "interaction_resolved":
      return true;
    case "runtime_extension_invalidated":
    case "elicitation_requested":
    case "interactions_cleared":
      return event.sessionId === null;
    default:
      return false;
  }
}

function failureSame(
  left: ApplicationError | null,
  right: ApplicationError | null,
): boolean {
  if (left === right) {
    return true;
  }
  if (left === null || right === null) {
    return false;
  }
  return (
    left.code === right.code &&
    left.diagnostic === right.diagnostic &&
    left.recoverable === right.recoverable
  );
}

function applicationError(
  value: unknown,
  fallbackDiagnostic: string,
  fallbackCode: ApplicationError["code"] = "connection_failed",
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
