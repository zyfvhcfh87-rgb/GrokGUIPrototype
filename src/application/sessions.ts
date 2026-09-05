import type {
  ApplicationError,
  RuntimeSessionCapabilities,
  RuntimeState,
  SessionState,
  SessionSummary,
} from "./contract.ts";

export type SessionListKind =
  | "idle"
  | "unavailable"
  | "loading"
  | "empty"
  | "ready"
  | "stale"
  | "failed";

export type SessionRowStatus =
  | "listed"
  | "creating"
  | "selected"
  | "closed"
  | "failed"
  | "stale";

export type SessionRow = SessionSummary & {
  status: SessionRowStatus;
};

export type SessionListPresentation = {
  kind: SessionListKind;
  heading: string;
  detail: string;
  canCreate: boolean;
  canRetry: boolean;
  canOpen: boolean;
  canClose: boolean;
};

export function sameWorkspacePath(left: string, right: string): boolean {
  if (left === right) {
    return true;
  }
  const foldedLeft = foldWorkspacePath(left);
  const foldedRight = foldWorkspacePath(right);
  if (foldedLeft === foldedRight) {
    return true;
  }
  return (
    looksLikeWindowsPath(foldedLeft) &&
    looksLikeWindowsPath(foldedRight) &&
    foldedLeft.toLowerCase() === foldedRight.toLowerCase()
  );
}

export function sessionBelongsToWorkspace(
  session: Pick<SessionSummary, "workspace">,
  workspace: string,
): boolean {
  return sameWorkspacePath(session.workspace, workspace);
}

export function isRuntimeUsable(state: RuntimeState): boolean {
  return state === "ready" || state === "working" || state === "waiting_for_input";
}

export function describeSessionList(input: {
  workspace: string | null;
  runtimeState: RuntimeState;
  capabilities: RuntimeSessionCapabilities | null;
  listKind: SessionListKind;
  sessionCount: number;
  failure: ApplicationError | null;
}): SessionListPresentation {
  const canCreate =
    input.workspace !== null &&
    isRuntimeUsable(input.runtimeState) &&
    (input.capabilities?.create ?? false);
  const canOpen =
    input.workspace !== null &&
    isRuntimeUsable(input.runtimeState) &&
    ((input.capabilities?.resume ?? false) || (input.capabilities?.load ?? false));
  const canClose =
    input.workspace !== null &&
    isRuntimeUsable(input.runtimeState) &&
    (input.capabilities?.close ?? false);

  if (input.workspace === null) {
    return presentation(
      "idle",
      "No workspace selected",
      "Choose a workspace to see its Grok sessions.",
      false,
      false,
      false,
      false,
    );
  }
  if (input.listKind === "loading") {
    return presentation(
      "loading",
      "Loading sessions",
      "Asking Grok for sessions in this workspace.",
      canCreate,
      false,
      canOpen,
      canClose,
    );
  }
  if (input.listKind === "failed") {
    return presentation(
      "failed",
      "Sessions unavailable",
      input.failure?.diagnostic ?? "The session list could not be loaded.",
      canCreate,
      input.failure?.recoverable ?? true,
      canOpen,
      canClose,
    );
  }
  if (input.listKind === "unavailable") {
    return presentation(
      "unavailable",
      "Sessions unavailable",
      isRuntimeUsable(input.runtimeState)
        ? "This runtime does not advertise session listing."
        : "Reconnect before listing sessions for this workspace.",
      canCreate,
      !isRuntimeUsable(input.runtimeState),
      canOpen,
      canClose,
    );
  }
  if (input.listKind === "stale") {
    return presentation(
      "stale",
      "Session list may be out of date",
      "Refresh to reload the sessions Grok still has for this workspace.",
      canCreate,
      true,
      canOpen,
      canClose,
    );
  }
  if (input.sessionCount === 0) {
    return presentation(
      "empty",
      "No sessions yet",
      "Create a session to start working in this workspace.",
      canCreate,
      false,
      canOpen,
      canClose,
    );
  }
  return presentation(
    "ready",
    "Sessions",
    `${input.sessionCount} session${input.sessionCount === 1 ? "" : "s"} in this workspace.`,
    canCreate,
    false,
    canOpen,
    canClose,
  );
}

export function describeSessionStatus(status: SessionRowStatus | SessionState): string {
  switch (status) {
    case "creating":
      return "Creating";
    case "listed":
    case "ready":
      return "Ready";
    case "selected":
      return "Selected";
    case "working":
      return "Working";
    case "waiting_for_input":
      return "Needs input";
    case "cancelling":
      return "Cancelling";
    case "completed":
      return "Completed";
    case "closed":
      return "Closed";
    case "failed":
      return "Failed";
    case "stale":
      return "Stale";
  }
}

function foldWorkspacePath(value: string): string {
  return value.replace(/[\\/]+$/, "");
}

function looksLikeWindowsPath(value: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(value) || value.startsWith("\\\\");
}

function presentation(
  kind: SessionListKind,
  heading: string,
  detail: string,
  canCreate: boolean,
  canRetry: boolean,
  canOpen: boolean,
  canClose: boolean,
): SessionListPresentation {
  return { kind, heading, detail, canCreate, canRetry, canOpen, canClose };
}
