import type {
  ApplicationError,
  RecentWorkspace,
  RuntimeState,
  SetupStatus,
} from "./contract.ts";

export type LaunchPresentation = {
  kind:
    | "checking"
    | "missing"
    | "invalid"
    | "connecting"
    | "authenticating"
    | "ready"
    | "working"
    | "waiting"
    | "incompatible"
    | "unsupported_authentication"
    | "failed"
    | "disconnected";
  label: string;
  heading: string;
  detail: string;
  canRetry: boolean;
};

export function describeLaunchState(input: {
  setup: SetupStatus | null;
  runtimeState: RuntimeState;
  failure: ApplicationError | null;
}): LaunchPresentation {
  if (input.setup === null) {
    if (input.failure !== null) {
      return presentation(
        "failed",
        "Setup failed",
        "The local setup check could not finish",
        input.failure.diagnostic,
        input.failure.recoverable,
      );
    }
    return presentation(
      "checking",
      "Checking setup",
      "Looking for Grok Build",
      "The app is checking your local Grok installation.",
      false,
    );
  }
  if (!input.setup.runtimeAvailable || input.setup.executableState !== "available") {
    if (input.setup.executableState === "missing") {
      return presentation(
        "missing",
        "Grok missing",
        "Install Grok Build to continue",
        "Install Grok Build, then restart this app so it can check again.",
        false,
      );
    }
    return presentation(
      "invalid",
      "Grok invalid",
      "The configured Grok executable cannot be used",
      input.failure?.diagnostic ??
        input.setup.failure?.diagnostic ??
        "Check the configured Grok executable and restart the app.",
      false,
    );
  }

  if (input.runtimeState === "failed") {
    if (input.failure?.code === "unsupported_protocol") {
      return presentation(
        "incompatible",
        "Incompatible",
        "This Grok Build version is not compatible",
        "Update Grok Build to a version that supports ACP v1, then restart the app.",
        false,
      );
    }
    if (input.failure?.code === "unsupported_authentication") {
      return presentation(
        "unsupported_authentication",
        "Sign-in unsupported",
        "Grok advertised no supported sign-in method",
        input.failure.diagnostic,
        input.failure.recoverable,
      );
    }
    return presentation(
      "failed",
      "Connection failed",
      "Grok Build could not get ready",
      input.failure?.diagnostic ?? "The runtime stopped before setup completed.",
      input.failure?.recoverable ?? true,
    );
  }

  switch (input.runtimeState) {
    case "connecting":
      return presentation(
        "connecting",
        "Connecting",
        "Starting Grok Build",
        "A contained local runtime is negotiating ACP v1.",
        false,
      );
    case "authenticating":
      return presentation(
        "authenticating",
        "Authenticating",
        "Signing in through Grok Build",
        "The app uses only an authentication method advertised by the runtime.",
        false,
      );
    case "ready":
      return presentation(
        "ready",
        "Ready",
        "Grok Build is ready",
        "Choose a workspace to begin a local session.",
        false,
      );
    case "working":
      return presentation(
        "working",
        "Working",
        "Grok Build is working",
        "The active turn is running locally.",
        false,
      );
    case "waiting_for_input":
      return presentation(
        "waiting",
        "Needs input",
        "Grok Build is waiting for you",
        "Review the pending request before the runtime can continue.",
        false,
      );
    case "disconnected":
      return presentation(
        "disconnected",
        "Disconnected",
        "Grok Build is not connected",
        "Reconnect to continue with this workspace.",
        true,
      );
  }
}

export function describeWorkspaceList(workspaces: RecentWorkspace[]) {
  const availableCount = workspaces.filter((workspace) => workspace.available).length;
  const staleCount = workspaces.length - availableCount;
  const kind =
    workspaces.length === 0
      ? "empty"
      : availableCount === 0
        ? "stale"
        : staleCount === 0
          ? "ready"
          : "mixed";
  return { kind, availableCount, staleCount } as const;
}

function presentation(
  kind: LaunchPresentation["kind"],
  label: string,
  heading: string,
  detail: string,
  canRetry: boolean,
): LaunchPresentation {
  return { kind, label, heading, detail, canRetry };
}
