import type {
  AuthenticationMethod,
  RuntimeCapabilities,
  RuntimeSnapshot,
  RuntimeState,
  SetupStatus,
} from "./contract.ts";

export const APP_IDENTITY = {
  name: "Grok Build GUI",
  version: "0.1.0",
} as const;

export type CapabilityStatus = "advertised" | "not_advertised" | "not_reported";

export type CompatibilityFeature = {
  id: string;
  label: string;
  status: CapabilityStatus;
  note: string;
};

export type CompatibilityReport = {
  generatedAt: string;
  app: { name: string; version: string };
  runtime: {
    state: RuntimeState | "unknown";
    executableState: SetupStatus["executableState"] | "not_reported";
    executableSource: NonNullable<SetupStatus["executableSource"]> | "not_reported";
    agentProduct: "grok_build" | "other" | "not_reported";
    agentVersion: string | null;
    protocolVersion: number | null;
  };
  authentication: {
    advertisedMethods: AuthenticationMethod[];
  };
  sessions: {
    create: boolean | null;
    prompt: boolean | null;
    cancel: boolean | null;
    list: boolean | null;
    load: boolean | null;
    resume: boolean | null;
    close: boolean | null;
  };
  models: {
    advertised: boolean;
    count: number;
    truncated: boolean;
  };
  features: CompatibilityFeature[];
};

const SENSITIVE_PATTERN =
  /auth\.json|stderr|sessionId|GROK_HOME|password|credential|\/home\/|\/Users\/|[A-Za-z]:\\/iu;

export function buildCompatibilityReport(input: {
  generatedAt?: string;
  setup: SetupStatus | null;
  snapshot: Pick<RuntimeSnapshot, "state" | "capabilities"> | null;
}): CompatibilityReport {
  const capabilities = input.snapshot?.capabilities ?? null;
  const sessions = capabilities?.sessions;
  const report: CompatibilityReport = {
    generatedAt: input.generatedAt ?? "1970-01-01T00:00:00.000Z",
    app: { name: APP_IDENTITY.name, version: APP_IDENTITY.version },
    runtime: {
      state: input.snapshot?.state ?? "unknown",
      executableState: input.setup?.executableState ?? "not_reported",
      executableSource: input.setup?.executableSource ?? "not_reported",
      agentProduct: capabilities?.agent.product ?? "not_reported",
      agentVersion: safeVersion(capabilities?.agent.version),
      protocolVersion: capabilities?.protocolVersion ?? null,
    },
    authentication: {
      advertisedMethods: capabilities?.authenticationMethods ?? [],
    },
    sessions: {
      create: sessions?.create ?? null,
      prompt: sessions?.prompt ?? null,
      cancel: sessions?.cancel ?? null,
      list: sessions?.list ?? null,
      load: sessions?.load ?? null,
      resume: sessions?.resume ?? null,
      close: sessions?.close ?? null,
    },
    models: {
      advertised: capabilities?.models !== null && capabilities?.models !== undefined,
      count: capabilities?.models?.availableModels.length ?? 0,
      truncated: capabilities?.models?.truncated ?? false,
    },
    features: describeFeatures(capabilities),
  };
  return report;
}

export function serializeCompatibilityReport(report: CompatibilityReport): string {
  return `${JSON.stringify(report, null, 2)}\n`;
}

export function compatibilityReportIsSafe(text: string): boolean {
  return !SENSITIVE_PATTERN.test(text);
}

export async function copyText(text: string): Promise<"copied" | "unavailable"> {
  const clipboard = globalThis.navigator?.clipboard;
  if (clipboard === undefined || typeof clipboard.writeText !== "function") {
    return "unavailable";
  }
  try {
    await clipboard.writeText(text);
    return "copied";
  } catch {
    return "unavailable";
  }
}

function describeFeatures(capabilities: RuntimeCapabilities | null): CompatibilityFeature[] {
  const session = capabilities?.sessions;
  const flag = (value: boolean | undefined): CapabilityStatus => {
    if (capabilities === null) {
      return "not_reported";
    }
    return value === true ? "advertised" : "not_advertised";
  };
  const noteFor = (status: CapabilityStatus, advertised: string, missing: string) => {
    if (status === "not_reported") {
      return "The runtime has not finished capability negotiation.";
    }
    return status === "advertised" ? advertised : missing;
  };

  return [
    {
      id: "protocol",
      label: "ACP protocol",
      status: capabilities === null ? "not_reported" : "advertised",
      note: capabilities === null
        ? "Protocol version appears after a successful initialize."
        : `Negotiated ACP v${capabilities.protocolVersion}.`,
    },
    {
      id: "session_list",
      label: "Session list",
      status: flag(session?.list),
      note: noteFor(
        flag(session?.list),
        "This runtime advertised session listing.",
        "Session listing was not advertised. The GUI will not invent a list.",
      ),
    },
    {
      id: "session_create",
      label: "New session",
      status: flag(session?.create),
      note: noteFor(
        flag(session?.create),
        "This runtime advertised session creation.",
        "Session creation was not advertised.",
      ),
    },
    {
      id: "session_resume",
      label: "Resume session",
      status: flag(session?.resume),
      note: noteFor(
        flag(session?.resume),
        "This runtime advertised session resume.",
        "Resume was not advertised.",
      ),
    },
    {
      id: "session_close",
      label: "Close session",
      status: flag(session?.close),
      note: noteFor(
        flag(session?.close),
        "This runtime advertised session close.",
        "Close was not advertised.",
      ),
    },
    {
      id: "prompt",
      label: "Prompts",
      status: flag(session?.prompt),
      note: noteFor(
        flag(session?.prompt),
        "This runtime advertised prompt turns.",
        "Prompting was not advertised.",
      ),
    },
    {
      id: "cancel",
      label: "Cancel turn",
      status: flag(session?.cancel),
      note: noteFor(
        flag(session?.cancel),
        "This runtime advertised turn cancellation.",
        "Cancel was not advertised.",
      ),
    },
    {
      id: "models",
      label: "Model catalog",
      status: capabilities === null
        ? "not_reported"
        : capabilities.models !== null
          ? "advertised"
          : "not_advertised",
      note: capabilities?.models !== null && capabilities !== null
        ? "Model choices come from the negotiated catalog, not a hard-coded list."
        : "No model catalog was advertised.",
    },
  ];
}

function safeVersion(value: string | null | undefined): string | null {
  if (value === null || value === undefined || value.length === 0 || value.length > 64) {
    return null;
  }
  if (SENSITIVE_PATTERN.test(value)) {
    return null;
  }
  return value;
}
