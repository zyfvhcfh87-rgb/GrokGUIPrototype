import type {
  ApplicationError,
  RuntimeDiagnostics,
  RuntimeProcessContainment,
  Workspace,
  WorkspaceChangeContent,
  WorkspaceChangeEntry,
  WorkspaceChangeStatus,
  WorkspaceChanges,
  WorkspaceRequest,
} from "./contract.ts";

export const WORKSPACE_CHANGE_DISCLAIMER =
  "Workspace changes are not automatically attributable to the current Grok session.";

export type WorkspaceHealthBridge = {
  inspectWorkspaceChanges(request: WorkspaceRequest): Promise<WorkspaceChanges>;
  runtimeDiagnostics(): Promise<RuntimeDiagnostics>;
};

export type WorkspaceChangesPresentation = {
  kind: WorkspaceChanges["kind"] | "idle" | "loading" | "failed";
  heading: string;
  detail: string;
  disclaimer: string;
  entries: WorkspaceChangeEntry[];
  truncated: boolean;
  omittedEntryCount: number;
  omittedLineCount: number;
  canRefresh: boolean;
};

export type RuntimeHealthPresentation = {
  heading: string;
  detail: string;
  workerRunning: boolean;
  consecutiveFailures: number;
  lastFailure: ApplicationError | null;
  stderrLines: number;
  stderrBytes: number;
  containmentLabel: string;
  canRecover: boolean;
};

export type WorkspaceHealthState = {
  workspace: string | null;
  changes: WorkspaceChanges | null;
  diagnostics: RuntimeDiagnostics | null;
  changesFailure: ApplicationError | null;
  diagnosticsFailure: ApplicationError | null;
  loadingChanges: boolean;
  loadingDiagnostics: boolean;
};

const INITIAL_STATE: WorkspaceHealthState = {
  workspace: null,
  changes: null,
  diagnostics: null,
  changesFailure: null,
  diagnosticsFailure: null,
  loadingChanges: false,
  loadingDiagnostics: false,
};

export function describeWorkspaceChanges(input: {
  workspace: string | null;
  changes: WorkspaceChanges | null;
  failure: ApplicationError | null;
  loading: boolean;
}): WorkspaceChangesPresentation {
  if (input.workspace === null) {
    return {
      kind: "idle",
      heading: "No workspace selected",
      detail: "Choose a workspace to inspect its Git status.",
      disclaimer: WORKSPACE_CHANGE_DISCLAIMER,
      entries: [],
      truncated: false,
      omittedEntryCount: 0,
      omittedLineCount: 0,
      canRefresh: false,
    };
  }
  if (input.loading) {
    return {
      kind: "loading",
      heading: "Reading workspace changes",
      detail: "Inspecting Git status inside the selected folder.",
      disclaimer: WORKSPACE_CHANGE_DISCLAIMER,
      entries: input.changes?.entries ?? [],
      truncated: input.changes?.truncated ?? false,
      omittedEntryCount: input.changes?.omittedEntryCount ?? 0,
      omittedLineCount: input.changes?.omittedLineCount ?? 0,
      canRefresh: false,
    };
  }
  if (input.failure !== null) {
    return {
      kind: "failed",
      heading: "Workspace changes unavailable",
      detail: input.failure.diagnostic,
      disclaimer: WORKSPACE_CHANGE_DISCLAIMER,
      entries: [],
      truncated: false,
      omittedEntryCount: 0,
      omittedLineCount: 0,
      canRefresh: input.failure.recoverable,
    };
  }
  if (input.changes === null) {
    return {
      kind: "idle",
      heading: "Workspace changes",
      detail: "Refresh to inspect the current folder.",
      disclaimer: WORKSPACE_CHANGE_DISCLAIMER,
      entries: [],
      truncated: false,
      omittedEntryCount: 0,
      omittedLineCount: 0,
      canRefresh: true,
    };
  }
  if (input.changes.kind === "not_a_repository") {
    return {
      kind: "not_a_repository",
      heading: "Not a Git repository",
      detail: "This folder has no Git metadata. Changes cannot be listed.",
      disclaimer: WORKSPACE_CHANGE_DISCLAIMER,
      entries: [],
      truncated: false,
      omittedEntryCount: 0,
      omittedLineCount: 0,
      canRefresh: true,
    };
  }
  if (input.changes.kind === "unavailable") {
    return {
      kind: "unavailable",
      heading: "Git inspection unavailable",
      detail: "Git is missing or could not inspect this folder safely.",
      disclaimer: WORKSPACE_CHANGE_DISCLAIMER,
      entries: [],
      truncated: false,
      omittedEntryCount: 0,
      omittedLineCount: 0,
      canRefresh: true,
    };
  }
  return {
    kind: "repository",
    heading: input.changes.entries.length === 0 ? "No workspace changes" : "Workspace changes",
    detail:
      input.changes.entries.length === 0
        ? "The selected folder has a clean Git worktree."
        : `${input.changes.entries.length} changed path${input.changes.entries.length === 1 ? "" : "s"} in this folder.`,
    disclaimer: WORKSPACE_CHANGE_DISCLAIMER,
    entries: input.changes.entries,
    truncated: input.changes.truncated,
    omittedEntryCount: input.changes.omittedEntryCount,
    omittedLineCount: input.changes.omittedLineCount,
    canRefresh: true,
  };
}

export function describeRuntimeHealth(input: {
  diagnostics: RuntimeDiagnostics | null;
  failure: ApplicationError | null;
  loading: boolean;
}): RuntimeHealthPresentation {
  if (input.loading && input.diagnostics === null) {
    return {
      heading: "Reading runtime health",
      detail: "Collecting bounded process diagnostics.",
      workerRunning: false,
      consecutiveFailures: 0,
      lastFailure: null,
      stderrLines: 0,
      stderrBytes: 0,
      containmentLabel: "Unknown",
      canRecover: false,
    };
  }
  if (input.diagnostics === null) {
    return {
      heading: "Runtime health unavailable",
      detail: input.failure?.diagnostic ?? "Diagnostics have not been loaded.",
      workerRunning: false,
      consecutiveFailures: 0,
      lastFailure: input.failure,
      stderrLines: 0,
      stderrBytes: 0,
      containmentLabel: "Unknown",
      canRecover: input.failure?.recoverable ?? true,
    };
  }
  const failed = input.diagnostics.state === "failed" || input.diagnostics.state === "disconnected";
  return {
    heading: "Runtime health",
    detail: healthDetail(input.diagnostics),
    workerRunning: input.diagnostics.workerRunning,
    consecutiveFailures: input.diagnostics.consecutiveFailures,
    lastFailure: input.diagnostics.lastFailure ?? input.failure,
    stderrLines: input.diagnostics.stderrLines,
    stderrBytes: input.diagnostics.stderrBytes,
    containmentLabel: describeContainment(input.diagnostics.processContainment),
    canRecover: failed,
  };
}

export function describeChangeStatus(status: WorkspaceChangeStatus): string {
  switch (status) {
    case "added":
      return "Added";
    case "modified":
      return "Modified";
    case "deleted":
      return "Deleted";
    case "renamed":
      return "Renamed";
    case "copied":
      return "Copied";
    case "unmerged":
      return "Unmerged";
    case "untracked":
      return "Untracked";
    case "ignored":
      return "Ignored";
    case "other":
      return "Other";
  }
}

export function describeChangeContent(content: WorkspaceChangeContent): string {
  switch (content) {
    case "text":
      return "Text";
    case "binary":
      return "Binary";
    case "omitted":
      return "Omitted";
    case "unavailable":
      return "Unavailable";
  }
}

export function createWorkspaceHealthController(bridge: WorkspaceHealthBridge) {
  let state = INITIAL_STATE;
  let changesToken = 0;
  let diagnosticsToken = 0;
  const listeners = new Set<(state: WorkspaceHealthState) => void>();

  const publish = (patch: Partial<WorkspaceHealthState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) {
      listener(state);
    }
  };

  const refreshChanges = async () => {
    const workspace = state.workspace;
    if (workspace === null) {
      publish({ changes: null, changesFailure: null, loadingChanges: false });
      return;
    }
    const token = ++changesToken;
    publish({ loadingChanges: true, changesFailure: null });
    try {
      const changes = await bridge.inspectWorkspaceChanges({ path: workspace });
      if (token !== changesToken || state.workspace !== workspace) {
        return;
      }
      publish({
        changes: { ...changes, attributableToSession: false },
        changesFailure: null,
        loadingChanges: false,
      });
    } catch (error) {
      if (token !== changesToken || state.workspace !== workspace) {
        return;
      }
      publish({
        changes: null,
        changesFailure: applicationError(error, "Workspace changes could not be inspected."),
        loadingChanges: false,
      });
    }
  };

  const refreshDiagnostics = async () => {
    const token = ++diagnosticsToken;
    publish({ loadingDiagnostics: true, diagnosticsFailure: null });
    try {
      const diagnostics = await bridge.runtimeDiagnostics();
      if (token !== diagnosticsToken) {
        return;
      }
      publish({
        diagnostics,
        diagnosticsFailure: null,
        loadingDiagnostics: false,
      });
    } catch (error) {
      if (token !== diagnosticsToken) {
        return;
      }
      publish({
        diagnostics: null,
        diagnosticsFailure: applicationError(error, "Runtime diagnostics could not be loaded."),
        loadingDiagnostics: false,
      });
    }
  };

  return {
    getState: () => state,
    subscribe: (listener: (state: WorkspaceHealthState) => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    setWorkspace: async (workspace: Workspace | null) => {
      const path = workspace?.path ?? null;
      if (path === state.workspace) {
        return;
      }
      changesToken += 1;
      publish({
        workspace: path,
        changes: null,
        changesFailure: null,
        loadingChanges: path !== null,
      });
      if (path !== null) {
        await refreshChanges();
      }
    },
    refreshChanges,
    refreshDiagnostics,
    dispose: () => {
      changesToken += 1;
      diagnosticsToken += 1;
      listeners.clear();
    },
  } as const;
}

function healthDetail(diagnostics: RuntimeDiagnostics): string {
  if (diagnostics.consecutiveFailures > 1) {
    return `Repeated process failure (${diagnostics.consecutiveFailures}). Recover restarts the same runtime without launching a second child.`;
  }
  if (diagnostics.state === "failed") {
    return "The Grok child stopped. Recover restarts it through the existing runtime.";
  }
  if (diagnostics.state === "disconnected") {
    return "The runtime is disconnected. Reconnect uses the existing GrokRuntime instance.";
  }
  if (diagnostics.workerRunning) {
    return "The contained ACP child is running. Stderr is counted, never shown.";
  }
  return "No ACP child is running.";
}

function describeContainment(value: RuntimeProcessContainment): string {
  return value === "windows_job" ? "Windows Job Object" : "Direct child process";
}

function applicationError(value: unknown, fallback: string): ApplicationError {
  if (
    typeof value === "object" &&
    value !== null &&
    typeof (value as ApplicationError).code === "string" &&
    typeof (value as ApplicationError).diagnostic === "string" &&
    typeof (value as ApplicationError).recoverable === "boolean"
  ) {
    return value as ApplicationError;
  }
  return { code: "protocol_request_failed", diagnostic: fallback, recoverable: true };
}
