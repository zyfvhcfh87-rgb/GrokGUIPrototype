import type {
  ApplicationError,
  RuntimeCapabilities,
  RuntimeState,
  SetupStatus,
  Workspace,
} from "./contract.ts";
import { describeLaunchState } from "./setup.ts";

export type OnboardingStepId =
  | "welcome"
  | "detect"
  | "authenticate"
  | "workspace"
  | "compatibility";

export type OnboardingStep = {
  id: OnboardingStepId;
  title: string;
  detail: string;
  status: "complete" | "current" | "upcoming" | "blocked";
};

export type OnboardingPresentation = {
  steps: OnboardingStep[];
  current: OnboardingStepId;
  canContinue: boolean;
  canFinish: boolean;
  finishLabel: string;
};

export function describeOnboarding(input: {
  step: OnboardingStepId;
  setup: SetupStatus | null;
  runtimeState: RuntimeState;
  failure: ApplicationError | null;
  selectedWorkspace: Workspace | null;
  capabilities: RuntimeCapabilities | null;
}): OnboardingPresentation {
  const launch = describeLaunchState({
    setup: input.setup,
    runtimeState: input.runtimeState,
    failure: input.failure,
  });
  const detected = input.setup?.executableState === "available" && input.setup.runtimeAvailable;
  const ready = input.runtimeState === "ready" || input.runtimeState === "working" ||
    input.runtimeState === "waiting_for_input";
  const workspaceSelected = input.selectedWorkspace !== null;

  const steps: OnboardingStep[] = [
    {
      id: "welcome",
      title: "Welcome",
      detail: "This is a local cockpit for the Grok Build already installed on this computer. It does not bundle Grok or read credential files.",
      status: statusFor("welcome", input.step),
    },
    {
      id: "detect",
      title: "Find Grok Build",
      detail: detected
        ? "Grok Build is available through the reviewed discovery path. The executable location stays in the native runtime."
        : launch.kind === "missing"
          ? "Install Grok Build, then choose Reconnect or restart this app. This guide will not install it for you."
          : launch.kind === "invalid"
            ? "The configured executable cannot be used. Fix the Grok install, then retry."
            : launch.detail,
      status: statusFor("detect", input.step, detected ? "complete" : launch.kind === "checking" ? "current" : "blocked"),
    },
    {
      id: "authenticate",
      title: "Sign in through Grok",
      detail: ready
        ? advertisedAuthDetail(input.capabilities)
        : input.runtimeState === "authenticating"
          ? "Grok Build is handling sign-in with an advertised method. This app never sees credential contents."
          : launch.kind === "unsupported_authentication"
            ? "Grok advertised no supported sign-in method."
            : "Sign-in starts only after Grok advertises a method. Cached sign-in is used when Grok already has it.",
      status: statusFor("authenticate", input.step, ready ? "complete" : undefined),
    },
    {
      id: "workspace",
      title: "Choose a workspace",
      detail: workspaceSelected
        ? "A canonical workspace is selected. Recent folders are remembered here; Grok still owns the sessions."
        : "Pick a local folder. The native picker validates it before the cockpit uses it.",
      status: statusFor("workspace", input.step, workspaceSelected ? "complete" : undefined),
    },
    {
      id: "compatibility",
      title: "Compatibility",
      detail: input.capabilities === null
        ? "A privacy-safe report is built after initialize. It will list advertised capabilities without paths, session IDs, or prompts."
        : `Negotiated ACP v${input.capabilities.protocolVersion}. Optional features appear only when this runtime advertised them.`,
      status: statusFor("compatibility", input.step, input.capabilities !== null ? "complete" : undefined),
    },
  ];

  return {
    steps,
    current: input.step,
    canContinue: canAdvance(input.step, detected, ready),
    canFinish: true,
    finishLabel: "Finish guide",
  };
}

export const ONBOARDING_STEP_IDS: OnboardingStepId[] = [
  "welcome",
  "detect",
  "authenticate",
  "workspace",
  "compatibility",
];

export function nextOnboardingStep(step: OnboardingStepId): OnboardingStepId {
  return ONBOARDING_STEP_IDS[Math.min(ONBOARDING_STEP_IDS.indexOf(step) + 1, ONBOARDING_STEP_IDS.length - 1)] ??
    "compatibility";
}

export function previousOnboardingStep(step: OnboardingStepId): OnboardingStepId {
  return ONBOARDING_STEP_IDS[Math.max(ONBOARDING_STEP_IDS.indexOf(step) - 1, 0)] ?? "welcome";
}

function statusFor(
  id: OnboardingStepId,
  current: OnboardingStepId,
  complete?: "complete" | "current" | "blocked",
): OnboardingStep["status"] {
  if (id === current) {
    return "current";
  }
  const order = ONBOARDING_STEP_IDS.indexOf(id);
  const currentIndex = ONBOARDING_STEP_IDS.indexOf(current);
  if (complete === "blocked" && order >= currentIndex) {
    return "blocked";
  }
  if (order < currentIndex || complete === "complete") {
    return "complete";
  }
  return "upcoming";
}

function canAdvance(step: OnboardingStepId, detected: boolean, ready: boolean): boolean {
  if (step === "compatibility") {
    return false;
  }
  if (step === "detect") {
    return detected;
  }
  if (step === "authenticate") {
    return ready;
  }
  return true;
}

function advertisedAuthDetail(capabilities: RuntimeCapabilities | null): string {
  const methods = capabilities?.authenticationMethods ?? [];
  if (methods.length === 0) {
    return "Grok is ready. No authentication methods were listed in the negotiated snapshot.";
  }
  return `Grok is ready. Advertised sign-in methods: ${methods.join(", ")}. Contents stay with Grok.`;
}
