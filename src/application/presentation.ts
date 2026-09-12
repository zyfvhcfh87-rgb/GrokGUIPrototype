import type {
  ApplicationError,
  OnboardingStatus,
  PresentationMotion,
  PresentationPreferences,
  PresentationTheme,
} from "./contract.ts";

export type {
  OnboardingStatus,
  PresentationMotion,
  PresentationPreferences,
  PresentationTheme,
};

export const MIN_PANEL_WIDTH = 200;
export const MAX_PANEL_WIDTH = 420;
export const DEFAULT_PROJECTS_WIDTH = 260;
export const DEFAULT_DETAILS_WIDTH = 280;

export const DEFAULT_PRESENTATION_PREFERENCES: PresentationPreferences = {
  theme: "system",
  motion: "system",
  projectsOpen: true,
  detailsOpen: true,
  projectsWidth: DEFAULT_PROJECTS_WIDTH,
  detailsWidth: DEFAULT_DETAILS_WIDTH,
  onboarding: "unseen",
};

export type PresentationBridge = {
  getPresentationPreferences(): Promise<PresentationPreferences>;
  setPresentationPreferences(
    request: PresentationPreferences,
  ): Promise<PresentationPreferences>;
};

export type PresentationState = {
  preferences: PresentationPreferences;
  resolvedTheme: "light" | "dark";
  reduceMotion: boolean;
  failure: ApplicationError | null;
  loaded: boolean;
};

export function clampPanelWidth(value: number): number {
  if (!Number.isFinite(value)) {
    return DEFAULT_PROJECTS_WIDTH;
  }
  return Math.min(MAX_PANEL_WIDTH, Math.max(MIN_PANEL_WIDTH, Math.round(value)));
}

export function sanitizePresentationPreferences(
  value: Partial<PresentationPreferences> | null | undefined,
): PresentationPreferences {
  const current = value ?? {};
  return {
    theme: isTheme(current.theme) ? current.theme : DEFAULT_PRESENTATION_PREFERENCES.theme,
    motion: isMotion(current.motion) ? current.motion : DEFAULT_PRESENTATION_PREFERENCES.motion,
    projectsOpen: typeof current.projectsOpen === "boolean"
      ? current.projectsOpen
      : DEFAULT_PRESENTATION_PREFERENCES.projectsOpen,
    detailsOpen: typeof current.detailsOpen === "boolean"
      ? current.detailsOpen
      : DEFAULT_PRESENTATION_PREFERENCES.detailsOpen,
    projectsWidth: clampPanelWidth(
      current.projectsWidth ?? DEFAULT_PRESENTATION_PREFERENCES.projectsWidth,
    ),
    detailsWidth: clampPanelWidth(
      current.detailsWidth ?? DEFAULT_PRESENTATION_PREFERENCES.detailsWidth,
    ),
    onboarding: isOnboarding(current.onboarding)
      ? current.onboarding
      : DEFAULT_PRESENTATION_PREFERENCES.onboarding,
  };
}

export function resolveTheme(
  theme: PresentationTheme,
  prefersDark = true,
): "light" | "dark" {
  if (theme === "light") {
    return "light";
  }
  if (theme === "dark") {
    return "dark";
  }
  return prefersDark ? "dark" : "light";
}

export function resolveMotion(motion: PresentationMotion, prefersReduce = false): boolean {
  return motion === "reduce" || (motion === "system" && prefersReduce);
}

export function applyDocumentAppearance(input: {
  theme: "light" | "dark";
  reduceMotion: boolean;
  document?: Pick<Document, "documentElement">;
}): void {
  const root = input.document?.documentElement ?? globalThis.document?.documentElement;
  if (root === undefined) {
    return;
  }
  root.dataset.theme = input.theme;
  root.dataset.motion = input.reduceMotion ? "reduce" : "system";
  root.style.colorScheme = input.theme;
}

export function mergePresentationPatch(
  current: PresentationPreferences,
  patch: Partial<PresentationPreferences>,
): PresentationPreferences {
  return sanitizePresentationPreferences({ ...current, ...patch });
}

export function presentationPreferencesEqual(
  left: PresentationPreferences,
  right: PresentationPreferences,
): boolean {
  return left.theme === right.theme
    && left.motion === right.motion
    && left.projectsOpen === right.projectsOpen
    && left.detailsOpen === right.detailsOpen
    && left.projectsWidth === right.projectsWidth
    && left.detailsWidth === right.detailsWidth
    && left.onboarding === right.onboarding;
}

export type PresentationPatchInput =
  | Partial<PresentationPreferences>
  | ((current: PresentationPreferences) => Partial<PresentationPreferences>);

export function createPresentationController(bridge: PresentationBridge) {
  let state: PresentationState = {
    preferences: DEFAULT_PRESENTATION_PREFERENCES,
    resolvedTheme: "dark",
    reduceMotion: false,
    failure: null,
    loaded: false,
  };
  const listeners = new Set<(state: PresentationState) => void>();
  let themeMedia: MediaQueryList | undefined;
  let motionMedia: MediaQueryList | undefined;
  let writeChain: Promise<void> = Promise.resolve();
  const refreshResolved = () => {
    apply(state.preferences, state.failure);
  };

  const publish = (patch: Partial<PresentationState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) {
      listener(state);
    }
  };

  const apply = (preferences: PresentationPreferences, failure: ApplicationError | null) => {
    const resolvedTheme = resolveTheme(preferences.theme, systemPrefersDark());
    const reduceMotion = resolveMotion(preferences.motion, systemPrefersReducedMotion());
    applyDocumentAppearance({ theme: resolvedTheme, reduceMotion });
    publish({ preferences, resolvedTheme, reduceMotion, failure, loaded: true });
  };

  const persist = async (preferences: PresentationPreferences) => {
    const next = sanitizePresentationPreferences(preferences);
    apply(next, state.failure);
    try {
      const saved = sanitizePresentationPreferences(
        await bridge.setPresentationPreferences(next),
      );
      apply(saved, null);
      return saved;
    } catch (error) {
      publish({
        failure: applicationError(error, "Appearance preferences could not be saved."),
      });
      return next;
    }
  };

  const enqueue = <T,>(work: () => Promise<T>): Promise<T> => {
    const job = writeChain.then(work);
    writeChain = job.then(
      () => undefined,
      () => undefined,
    );
    return job;
  };

  const persistPatch = (patch: PresentationPatchInput): Promise<PresentationPreferences> =>
    enqueue(async () => {
      const current = state.preferences;
      const resolved = typeof patch === "function" ? patch(current) : patch;
      const next = mergePresentationPatch(current, resolved);
      if (presentationPreferencesEqual(next, current) && state.loaded) {
        return current;
      }
      return persist(next);
    });

  const attachMediaListeners = () => {
    themeMedia?.removeEventListener("change", refreshResolved);
    motionMedia?.removeEventListener("change", refreshResolved);
    themeMedia = globalThis.matchMedia?.("(prefers-color-scheme: dark)");
    motionMedia = globalThis.matchMedia?.("(prefers-reduced-motion: reduce)");
    themeMedia?.addEventListener("change", refreshResolved);
    motionMedia?.addEventListener("change", refreshResolved);
  };

  return {
    getState: () => state,
    subscribe: (listener: (state: PresentationState) => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    initialize: async () => {
      await enqueue(async () => {
        try {
          const loaded = sanitizePresentationPreferences(
            await bridge.getPresentationPreferences(),
          );
          if (!state.loaded) {
            apply(loaded, null);
          }
        } catch (error) {
          if (!state.loaded) {
            apply(
              DEFAULT_PRESENTATION_PREFERENCES,
              applicationError(error, "Appearance preferences could not be loaded."),
            );
          }
        }
      });
      attachMediaListeners();
    },
    update: persistPatch,
    setOnboarding: (onboarding: OnboardingStatus) => persistPatch({ onboarding }),
    dispose: () => {
      themeMedia?.removeEventListener("change", refreshResolved);
      motionMedia?.removeEventListener("change", refreshResolved);
      themeMedia = undefined;
      motionMedia = undefined;
      listeners.clear();
    },
  } as const;
}

function isTheme(value: unknown): value is PresentationTheme {
  return value === "system" || value === "light" || value === "dark";
}

function isMotion(value: unknown): value is PresentationMotion {
  return value === "system" || value === "reduce";
}

function isOnboarding(value: unknown): value is OnboardingStatus {
  return value === "unseen" || value === "skipped" || value === "completed";
}

function systemPrefersDark(): boolean {
  return globalThis.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true;
}

function systemPrefersReducedMotion(): boolean {
  return globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
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
  return { code: "preferences_unavailable", diagnostic: fallback, recoverable: true };
}
