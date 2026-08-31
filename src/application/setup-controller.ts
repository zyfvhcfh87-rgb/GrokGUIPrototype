import type {
  ApplicationError,
  ApplicationEventEnvelope,
  RecentWorkspace,
  RecentWorkspaceList,
  RuntimeSnapshot,
  RuntimeState,
  SetupStatus,
  Workspace,
  WorkspaceRequest,
} from "./contract.ts";
import { initialApplicationState, reduceApplicationEvent } from "./state.ts";

type Unlisten = () => void;

export type SetupControllerBridge = {
  onEvent(listener: (event: ApplicationEventEnvelope) => void): Promise<Unlisten>;
  setupStatus(): Promise<SetupStatus>;
  listRecentWorkspaces(): Promise<RecentWorkspaceList>;
  runtimeSnapshot(): Promise<RuntimeSnapshot>;
  startRuntime?(): Promise<RuntimeSnapshot>;
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

export function createSetupController(bridge: SetupControllerBridge) {
  let state = INITIAL_STATE;
  let orderedState = initialApplicationState();
  let unlisten: Unlisten | null = null;
  let lifecycle = 0;
  const listeners = new Set<(state: SetupControllerState) => void>();

  const publish = (patch: Partial<SetupControllerState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) {
      listener(state);
    }
  };

  const handleEvent = (envelope: ApplicationEventEnvelope) => {
    const previous = orderedState;
    orderedState = reduceApplicationEvent(orderedState, envelope);
    if (orderedState === previous || orderedState.runtime === previous.runtime) {
      return;
    }
    const orderedFailure = orderedState.runtime.failure;
    publish({
      runtimeState: orderedState.runtime.state,
      failure:
        orderedFailure === null
          ? orderedState.runtime.state === "failed"
            ? state.failure
            : null
          : {
              code: "connection_failed",
              diagnostic: orderedFailure.diagnostic,
              recoverable: orderedFailure.recoverable,
            },
    });
  };

  const acceptSnapshot = (snapshot: RuntimeSnapshot) => {
    const snapshotIsCurrent =
      snapshot.generation > orderedState.generation ||
      (snapshot.generation === orderedState.generation &&
        snapshot.lastSequence >= orderedState.lastSequence);
    if (snapshotIsCurrent) {
      const snapshotState = initialApplicationState(snapshot.generation);
      orderedState = {
        ...snapshotState,
        lastSequence: snapshot.lastSequence,
        runtime: {
          ...snapshotState.runtime,
          state: snapshot.state,
        },
      };
    }
    return orderedState.runtime.state;
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

  const connect = async () => {
    if (bridge.startRuntime === undefined) {
      return;
    }
    try {
      const snapshot = await bridge.startRuntime();
      publish({
        runtimeState: acceptSnapshot(snapshot),
        capabilities: snapshot.capabilities,
        failure: null,
      });
    } catch (error) {
      publish({
        runtimeState: "failed",
        failure: applicationError(error, "Grok Build could not be started."),
      });
    }
  };

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
    retry: connect,
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
      listeners.clear();
    },
  } as const;
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
