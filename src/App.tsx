import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import type { ApplicationTransport } from "./application/bridge.ts";
import { createApplicationBridge } from "./application/bridge.ts";
import { buildCompatibilityReport } from "./application/compatibility.ts";
import { createConversationController } from "./application/conversation-controller.ts";
import { shortSessionId, workspaceName } from "./application/display.ts";
import { createInteractionController } from "./application/interaction-controller.ts";
import type { InteractionContext } from "./application/interactions.ts";
import { pendingInteractions } from "./application/interactions.ts";
import {
  describeOnboarding,
  type OnboardingStepId,
} from "./application/onboarding.ts";
import { createPresentationController } from "./application/presentation.ts";
import { createSessionController } from "./application/session-controller.ts";
import {
  describeSessionList,
} from "./application/sessions.ts";
import { describeLaunchState, describeRecovery, describeWorkspaceList } from "./application/setup.ts";
import { createSetupController } from "./application/setup-controller.ts";
import { matchShortcut, shortcutConsumesEvent } from "./application/shortcuts.ts";
import { createTauriTransport } from "./application/tauri.ts";
import {
  createWorkspaceHealthController,
  describeRuntimeHealth,
  describeWorkspaceChanges,
} from "./application/workspace-health.ts";
import appIcon from "./assets/app-icon.svg";
import { ActivityPane } from "./cockpit/ActivityPane.tsx";
import { AppearanceDialog } from "./cockpit/AppearanceDialog.tsx";
import { CompatibilityDialog } from "./cockpit/CompatibilityDialog.tsx";
import { ConversationPane } from "./cockpit/ConversationPane.tsx";
import { InteractionPane } from "./cockpit/InteractionPane.tsx";
import { OnboardingDialog } from "./cockpit/OnboardingDialog.tsx";
import { PanelResize } from "./cockpit/PanelResize.tsx";
import { ShortcutsDialog } from "./cockpit/ShortcutsDialog.tsx";
import { Titlebar } from "./cockpit/Titlebar.tsx";
import { WorkspaceHealthPane } from "./cockpit/WorkspaceHealthPane.tsx";
import { WorkspacePanel } from "./cockpit/WorkspacePanel.tsx";

export type AppProps = {
  transport?: ApplicationTransport;
  autoWorkspace?: string;
  autoOpenFirstSession?: boolean;
};

const AUTH_LABELS = {
  cached_token: "Cached sign-in",
  grok_com: "Grok.com",
  xai_api_key: "xAI API key",
  grok: "Grok",
  other: "Other",
} as const;

const SOURCE_LABELS = {
  configured: "Configured location",
  user_install: "Default user install",
  path: "System PATH",
} as const;

type Overlay = "onboarding" | "shortcuts" | "appearance" | "compatibility";

export function App({
  transport,
  autoWorkspace,
  autoOpenFirstSession = false,
}: AppProps = {}) {
  const [sessionFocus, setSessionFocus] = useState(0);
  const [overlay, setOverlay] = useState<Overlay | null>(null);
  const [onboardingStep, setOnboardingStep] = useState<OnboardingStepId>("welcome");
  const [liveMessage, setLiveMessage] = useState("Checking setup");
  const pendingProjectsFocus = useRef(false);
  const bridge = useMemo(
    () => createApplicationBridge(transport ?? createTauriTransport()),
    [transport],
  );
  const controller = useMemo(() => createSetupController(bridge), [bridge]);
  const sessions = useMemo(() => createSessionController(bridge), [bridge]);
  const conversation = useMemo(() => createConversationController(bridge), [bridge]);
  const health = useMemo(() => createWorkspaceHealthController(bridge), [bridge]);
  const presentation = useMemo(() => createPresentationController(bridge), [bridge]);
  const state = useSyncExternalStore(controller.subscribe, controller.getState);
  const sessionState = useSyncExternalStore(sessions.subscribe, sessions.getState);
  const conversationState = useSyncExternalStore(conversation.subscribe, conversation.getState);
  const healthState = useSyncExternalStore(health.subscribe, health.getState);
  const presentationState = useSyncExternalStore(presentation.subscribe, presentation.getState);
  const prefs = presentationState.preferences;
  const readInteractionContext = useMemo((): (() => InteractionContext) => () => {
    const current = conversation.getState();
    const navigation = sessions.getState();
    const setup = controller.getState();
    return {
      application: current.application,
      sessionId: current.sessionId,
      epoch: current.interactionEpoch,
      blocked: current.cancelling || navigation.opening || navigation.closing || navigation.creating ||
        setup.initializing || setup.choosingWorkspace || current.sessionId !== navigation.selectedSessionId ||
        !["ready", "working", "waiting_for_input"].includes(setup.runtimeState),
    };
  }, [controller, conversation, sessions]);
  const interactions = useMemo(
    () => createInteractionController(bridge, readInteractionContext),
    [bridge, readInteractionContext],
  );
  const interactionStatuses = useSyncExternalStore(interactions.subscribe, interactions.getState);
  useEffect(() => {
    interactions.reconcile();
  }, [interactions, conversationState, sessionState, state]);
  const interactionPane = (
    <InteractionPane
      context={readInteractionContext()}
      controller={interactions}
      statuses={interactionStatuses}
    />
  );

  useEffect(() => {
    void controller.initialize();
    void sessions.initialize();
    void conversation.initialize();
    void health.refreshDiagnostics();
    void presentation.initialize();
    return () => {
      controller.dispose();
      sessions.dispose();
      conversation.dispose();
      health.dispose();
      presentation.dispose();
    };
  }, [controller, conversation, health, presentation, sessions]);

  useEffect(() => {
    sessions.setRuntime(state.runtimeState, state.capabilities);
    conversation.setRuntime(state.runtimeState, state.capabilities);
  }, [conversation, sessions, state.capabilities, state.runtimeState]);

  const restoreSessionId =
    state.selectedWorkspace === null
      ? null
      : (state.recentWorkspaces.find(
          (workspace) => workspace.path === state.selectedWorkspace?.path,
        )?.lastSessionId ?? null);

  useEffect(() => {
    void sessions.setWorkspace(state.selectedWorkspace, restoreSessionId);
    setSessionFocus(0);
  }, [restoreSessionId, sessions, state.selectedWorkspace]);

  useEffect(() => {
    conversation.setSession(sessionState.selectedSession);
  }, [conversation, sessionState.selectedSession]);

  useEffect(() => {
    void health.setWorkspace(state.selectedWorkspace);
  }, [health, state.selectedWorkspace]);

  useEffect(() => {
    if (
      state.runtimeState === "failed" ||
      state.runtimeState === "disconnected" ||
      state.runtimeState === "ready"
    ) {
      void health.refreshDiagnostics();
    }
  }, [health, state.runtimeState]);

  const autoWorkspaceStarted = useRef(false);
  const autoSessionStarted = useRef(false);

  useEffect(() => {
    if (autoWorkspace === undefined || autoWorkspaceStarted.current || state.selectedWorkspace !== null) {
      return;
    }
    autoWorkspaceStarted.current = true;
    void controller.openRecent(autoWorkspace).catch(() => undefined);
  }, [autoWorkspace, controller, state.selectedWorkspace]);

  useEffect(() => {
    const first = sessionState.sessions[0];
    if (
      !autoOpenFirstSession ||
      autoSessionStarted.current ||
      sessionState.selectedSessionId !== null ||
      first === undefined
    ) {
      return;
    }
    autoSessionStarted.current = true;
    void sessions.openSession(first.sessionId).catch(() => undefined);
  }, [autoOpenFirstSession, sessionState.selectedSessionId, sessionState.sessions, sessions]);

  useEffect(() => {
    if (presentationState.loaded && prefs.onboarding === "unseen") {
      setOverlay((current) => current ?? "onboarding");
    }
  }, [presentationState.loaded, prefs.onboarding]);

  useEffect(() => {
    if (!pendingProjectsFocus.current || !prefs.projectsOpen) {
      return;
    }
    pendingProjectsFocus.current = false;
    document.getElementById("workspace-heading")?.focus();
  }, [prefs.projectsOpen]);

  const launch = describeLaunchState({
    setup: state.setup,
    runtimeState: state.runtimeState,
    failure: state.failure,
  });
  const recovery = describeRecovery({
    setup: state.setup,
    runtimeState: state.runtimeState,
    failure: state.failure,
    workspaceFailure: state.workspaceFailure,
    sessionFailure: sessionState.failure,
  });

  useEffect(() => {
    const pending = pendingInteractions(readInteractionContext()).length;
    if (pending > 0) {
      setLiveMessage(
        `${pending} pending request${pending === 1 ? "" : "s"} need review. Approval is never implied by color.`,
      );
      return;
    }
    setLiveMessage(`Runtime status: ${launch.label}.`);
  }, [launch.label, readInteractionContext, conversationState, sessionState, state]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const id = matchShortcut(event);
      if (id === null) {
        return;
      }
      const target = event.target instanceof HTMLElement ? event.target : null;
      if (!shortcutConsumesEvent(id, target)) {
        return;
      }
      if (overlay !== null && id !== "cancel-or-dismiss" && id !== "shortcuts" &&
        id !== "appearance" && id !== "onboarding" && id !== "compatibility") {
        return;
      }
      event.preventDefault();
      switch (id) {
        case "toggle-projects":
          restoreFocusIfInside("workspace-panel", "toggle-projects");
          void presentation.update({ ...prefs, projectsOpen: !prefs.projectsOpen });
          break;
        case "toggle-details":
          restoreFocusIfInside("details-panel", "toggle-details");
          void presentation.update({ ...prefs, detailsOpen: !prefs.detailsOpen });
          break;
        case "focus-projects":
          pendingProjectsFocus.current = true;
          if (!prefs.projectsOpen) {
            void presentation.update({ ...prefs, projectsOpen: true });
            break;
          }
          pendingProjectsFocus.current = false;
          document.getElementById("workspace-heading")?.focus();
          break;
        case "focus-conversation":
          document.getElementById("main-panel")?.focus();
          break;
        case "focus-composer":
          document.getElementById("conversation-composer")?.focus();
          break;
        case "new-session":
          if (state.selectedWorkspace !== null) {
            void sessions.createSession().catch(() => undefined);
          }
          break;
        case "send-prompt":
          void conversation.sendPrompt().catch(() => undefined);
          break;
        case "cancel-or-dismiss":
          if (overlay !== null) {
            setOverlay(null);
            break;
          }
          void conversation.cancelPrompt().catch(() => undefined);
          break;
        case "appearance":
          setOverlay("appearance");
          break;
        case "shortcuts":
          setOverlay("shortcuts");
          break;
        case "onboarding":
          setOverlay("onboarding");
          setOnboardingStep("welcome");
          break;
        case "compatibility":
          setOverlay("compatibility");
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [conversation, overlay, prefs, presentation, sessions, state.selectedWorkspace]);

  const recent = describeWorkspaceList(state.recentWorkspaces);
  const conversationView = conversation.presentation();
  const hasSession = sessionState.selectedSessionId !== null;
  const sessionList = describeSessionList({
    workspace: sessionState.workspace,
    runtimeState: sessionState.runtimeState,
    capabilities: sessionState.capabilities,
    listKind: sessionState.listKind,
    sessionCount: sessionState.sessions.length,
    failure: sessionState.failure,
    nextCursor: sessionState.nextCursor,
  });
  const agent = state.capabilities?.agent;
  const authMethods = state.capabilities?.authenticationMethods ?? [];
  const changesView = describeWorkspaceChanges({
    workspace: healthState.workspace,
    changes: healthState.changes,
    failure: healthState.changesFailure,
    loading: healthState.loadingChanges,
  });
  const healthView = describeRuntimeHealth({
    diagnostics: healthState.diagnostics,
    failure: healthState.diagnosticsFailure ?? state.failure,
    loading: healthState.loadingDiagnostics,
  });
  const healthPane = (
    <WorkspaceHealthPane
      changes={changesView}
      health={healthView}
      onRefreshChanges={() => void health.refreshChanges()}
      onRefreshDiagnostics={() => void health.refreshDiagnostics()}
      onRecover={() => {
        void controller.retry();
        void health.refreshDiagnostics();
      }}
    />
  );
  const compatibilityReport = buildCompatibilityReport({
    generatedAt: "1970-01-01T00:00:00.000Z",
    setup: state.setup,
    snapshot: {
      state: state.runtimeState,
      capabilities: state.capabilities,
    },
  });
  const onboarding = describeOnboarding({
    step: onboardingStep,
    setup: state.setup,
    runtimeState: state.runtimeState,
    failure: state.failure,
    selectedWorkspace: state.selectedWorkspace,
    capabilities: state.capabilities,
  });
  const cockpitClassName = [
    "cockpit",
    !prefs.projectsOpen && "cockpit--projects-collapsed",
    !prefs.detailsOpen && "cockpit--details-collapsed",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-panel">
        Skip to conversation
      </a>
      <div className="sr-only" aria-live="polite" aria-atomic="true">
        {liveMessage}
      </div>
      <Titlebar
        launch={launch}
        brandMark={appIcon}
        projectsOpen={prefs.projectsOpen}
        detailsOpen={prefs.detailsOpen}
        onToggleProjects={() => {
          restoreFocusIfInside("workspace-panel", "toggle-projects");
          void presentation.update({ ...prefs, projectsOpen: !prefs.projectsOpen });
        }}
        onToggleDetails={() => {
          restoreFocusIfInside("details-panel", "toggle-details");
          void presentation.update({ ...prefs, detailsOpen: !prefs.detailsOpen });
        }}
        onOpenAppearance={() => setOverlay("appearance")}
        onOpenShortcuts={() => setOverlay("shortcuts")}
        onOpenOnboarding={() => {
          setOnboardingStep("welcome");
          setOverlay("onboarding");
        }}
        onOpenCompatibility={() => setOverlay("compatibility")}
      />

      <main
        className={cockpitClassName}
        aria-label={hasSession ? "Grok Build conversation" : "Grok Build setup"}
        style={{
          ["--projects-width" as string]: `${prefs.projectsWidth}px`,
          ["--details-width" as string]: `${prefs.detailsWidth}px`,
        }}
      >
        {prefs.projectsOpen ? (
          <WorkspacePanel
            recentKind={recent.kind}
            staleCount={recent.staleCount}
            recentWorkspaces={state.recentWorkspaces}
            choosingWorkspace={state.choosingWorkspace}
            runtimeAvailable={state.setup?.runtimeAvailable === true}
            selectedWorkspace={state.selectedWorkspace}
            sessionList={sessionList}
            sessions={sessionState.sessions}
            sessionFocus={sessionFocus}
            creating={sessionState.creating}
            opening={sessionState.opening}
            closing={sessionState.closing}
            loadingMore={sessionState.loadingMore}
            onPickWorkspace={() => void controller.pickWorkspace().catch(() => undefined)}
            onOpenRecent={(path) => void controller.openRecent(path).catch(() => undefined)}
            onRemoveRecent={(path) => void controller.removeRecent(path).catch(() => undefined)}
            onCreateSession={() => void sessions.createSession().catch(() => undefined)}
            onOpenSession={(sessionId) => void sessions.openSession(sessionId).catch(() => undefined)}
            onCloseSession={(sessionId) => void sessions.closeSession(sessionId).catch(() => undefined)}
            onRetrySessions={() => void sessions.refresh()}
            onLoadMore={() => void sessions.loadMore()}
            onSessionFocus={setSessionFocus}
          />
        ) : null}
        {prefs.projectsOpen ? (
          <PanelResize
            label="Resize projects panel"
            cssVariable="--projects-width"
            value={prefs.projectsWidth}
            onChange={(projectsWidth) => void presentation.update({ ...prefs, projectsWidth })}
          />
        ) : null}

        <section
          id="main-panel"
          tabIndex={-1}
          className={[
            "shell-panel",
            "shell-panel--main",
            hasSession && "shell-panel--conversation",
          ]
            .filter(Boolean)
            .join(" ")}
          aria-labelledby={hasSession ? "conversation-heading" : "launch-heading"}
        >
          {hasSession ? (
            <ConversationPane
              interactions={interactionPane}
              presentation={conversationView}
              draft={conversationState.draft}
              onDraftChange={conversation.setDraft}
              onSend={() => void conversation.sendPrompt().catch(() => undefined)}
              onCancel={() => void conversation.cancelPrompt().catch(() => undefined)}
              onRecover={() => void controller.retry()}
              onOpenUrl={(href) => void conversation.openExternalUrl(href).catch(() => undefined)}
            />
          ) : (
            <>
              {interactionPane}
              <div className={`launch-card launch-card--${launch.kind}`}>
                <div className="launch-card__icon" aria-hidden="true">
                  <img src={appIcon} alt="" />
                  <span />
                </div>
                <p className="shell-panel__eyebrow">Local setup</p>
                <h1 id="launch-heading">{launch.heading}</h1>
                <p>{launch.detail}</p>
                {recovery !== null ? (
                  <p className={`recovery-copy recovery-copy--${recovery.kind}`}>
                    <strong>{recovery.title}. </strong>
                    {recovery.detail}
                  </p>
                ) : null}

                {launch.canRetry ? (
                  <button
                    className="button button--primary"
                    type="button"
                    onClick={() => void controller.retry()}
                  >
                    Reconnect
                  </button>
                ) : null}

                {sessionState.selectedSessionId !== null ? (
                  <div className="selected-session" aria-label="Selected session">
                    <span className="selected-workspace__check" aria-hidden="true">
                      ✓
                    </span>
                    <span>
                      <small>Selected session</small>
                      <strong>
                        {sessionState.sessions.find(
                          (item) => item.sessionId === sessionState.selectedSessionId,
                        )?.title ?? shortSessionId(sessionState.selectedSessionId)}
                      </strong>
                      <code>{sessionState.selectedSessionId}</code>
                    </span>
                  </div>
                ) : null}

                {state.selectedWorkspace !== null ? (
                  <div className="selected-workspace" aria-label="Selected workspace">
                    <span className="selected-workspace__check" aria-hidden="true">
                      ✓
                    </span>
                    <span>
                      <small>Selected workspace</small>
                      <strong>{workspaceName(state.selectedWorkspace.path)}</strong>
                      <code>{state.selectedWorkspace.path}</code>
                    </span>
                  </div>
                ) : launch.kind === "ready" ? (
                  <button
                    className="button button--primary"
                    type="button"
                    onClick={() => void controller.pickWorkspace().catch(() => undefined)}
                  >
                    Choose a workspace
                  </button>
                ) : null}
              </div>

              {state.workspaceFailure !== null ? (
                <div className="inline-alert" role="alert">
                  <strong>Workspace needs attention</strong>
                  <span>{state.workspaceFailure.diagnostic}</span>
                </div>
              ) : null}

              {sessionState.failure !== null ? (
                <div className="inline-alert" role="alert">
                  <strong>Session needs attention</strong>
                  <span>{sessionState.failure.diagnostic}</span>
                </div>
              ) : null}
            </>
          )}
        </section>

        {prefs.detailsOpen ? (
          <PanelResize
            label="Resize details panel"
            cssVariable="--details-width"
            value={prefs.detailsWidth}
            invert
            onChange={(detailsWidth) => void presentation.update({ ...prefs, detailsWidth })}
          />
        ) : null}

        {prefs.detailsOpen ? (
          <aside
            id="details-panel"
            className="shell-panel details-panel"
            aria-labelledby={hasSession ? "activity-heading" : "details-heading"}
          >
            {hasSession ? (
              <ActivityPane
                presentation={conversationView}
                controlBusy={conversationState.controlBusy}
                health={healthPane}
                onModelChange={(modelId) =>
                  void conversation
                    .setModel(modelId, conversationView.controls.currentReasoningValue)
                    .catch(() => undefined)
                }
                onReasoningChange={(value) => {
                  const modelId = conversationView.controls.currentModelId;
                  if (modelId !== null) {
                    void conversation.setModel(modelId, value).catch(() => undefined);
                  }
                }}
                onModeChange={(modeId) => void conversation.setMode(modeId).catch(() => undefined)}
                onConfigChange={(configId, value) =>
                  void conversation.setConfig(configId, value).catch(() => undefined)
                }
                onInsertCommand={conversation.insertCommand}
                onApprovePlan={() => void conversation.reviewPlan("approve").catch(() => undefined)}
                onRevisePlan={() => void conversation.reviewPlan("revise").catch(() => undefined)}
              />
            ) : (
              <>
                <p className="shell-panel__eyebrow">Connection</p>
                <h1 id="details-heading">Runtime details</h1>

                <dl className="detail-list">
                  <div>
                    <dt>Executable</dt>
                    <dd>{state.setup?.executableState ?? "Checking"}</dd>
                  </div>
                  <div>
                    <dt>Found through</dt>
                    <dd>
                      {state.setup?.executableSource
                        ? SOURCE_LABELS[state.setup.executableSource]
                        : "Not available"}
                    </dd>
                  </div>
                  <div>
                    <dt>Agent</dt>
                    <dd>
                      {agent?.product === "grok_build"
                        ? "Grok Build"
                        : state.capabilities
                          ? "Compatible ACP agent"
                          : "Waiting"}
                    </dd>
                  </div>
                  <div>
                    <dt>Version</dt>
                    <dd>{agent?.version ?? (state.capabilities ? "Not advertised" : "—")}</dd>
                  </div>
                  <div>
                    <dt>Protocol</dt>
                    <dd>
                      {state.capabilities ? `ACP v${state.capabilities.protocolVersion}` : "—"}
                    </dd>
                  </div>
                  <div>
                    <dt>Sessions</dt>
                    <dd>
                      {state.capabilities
                        ? sessionCapabilityLabel(state.capabilities.sessions)
                        : "—"}
                    </dd>
                  </div>
                </dl>

                <div className="auth-card">
                  <div className="auth-card__heading">
                    <span aria-hidden="true">◇</span>
                    <div>
                      <strong>Authentication</strong>
                      <small>Handled by Grok Build</small>
                    </div>
                  </div>
                  {authMethods.length > 0 ? (
                    <div className="chip-list" aria-label="Advertised authentication methods">
                      {authMethods.map((method) => (
                        <span key={method}>{AUTH_LABELS[method]}</span>
                      ))}
                    </div>
                  ) : (
                    <p>Methods appear after Grok advertises them.</p>
                  )}
                </div>

                <p className="privacy-note">
                  <span aria-hidden="true">⌾</span>
                  Credential contents stay with Grok and never cross into this interface.
                </p>
                {healthPane}
              </>
            )}
          </aside>
        ) : null}
      </main>

      <footer className="statusbar">
        <span>Local assets only</span>
        <span aria-hidden="true">·</span>
        <span>Native folder picker through a reviewed Rust command</span>
        <span aria-hidden="true">·</span>
        <span>Streaming conversation through GrokRuntime</span>
        <span aria-hidden="true">·</span>
        <span>No credential-file access</span>
      </footer>

      {overlay === "onboarding" ? (
        <OnboardingDialog
          presentation={onboarding}
          recovery={recovery}
          onStep={setOnboardingStep}
          onSkip={() => {
            void presentation.setOnboarding("skipped");
            setOverlay(null);
          }}
          onFinish={() => {
            void presentation.setOnboarding("completed");
            setOverlay(null);
          }}
          onPickWorkspace={() => void controller.pickWorkspace().catch(() => undefined)}
          onRetry={() => void controller.retry()}
        />
      ) : null}
      {overlay === "shortcuts" ? <ShortcutsDialog onClose={() => setOverlay(null)} /> : null}
      {overlay === "appearance" ? (
        <AppearanceDialog
          preferences={prefs}
          onChange={(next) => void presentation.update(next)}
          onClose={() => setOverlay(null)}
        />
      ) : null}
      {overlay === "compatibility" ? (
        <CompatibilityDialog
          report={{
            ...compatibilityReport,
            generatedAt: new Date().toISOString(),
          }}
          onClose={() => setOverlay(null)}
        />
      ) : null}
    </div>
  );
}

function restoreFocusIfInside(panelId: string, toggleId: string): void {
  const panel = document.getElementById(panelId);
  const active = document.activeElement;
  if (panel !== null && active instanceof HTMLElement && panel.contains(active)) {
    document.getElementById(toggleId)?.focus();
  }
}

function sessionCapabilityLabel(sessions: {
  create: boolean;
  list: boolean;
  load: boolean;
  resume: boolean;
  close: boolean;
}): string {
  const available = [
    sessions.list ? "list" : null,
    sessions.create ? "new" : null,
    sessions.load ? "load" : null,
    sessions.resume ? "resume" : null,
    sessions.close ? "close" : null,
  ].filter(Boolean);
  return available.length > 0 ? available.join(" · ") : "None advertised";
}
