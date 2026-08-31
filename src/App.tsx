import { useEffect, useMemo, useState, useSyncExternalStore } from "react";

import { createApplicationBridge } from "./application/bridge.ts";
import { createSetupController } from "./application/setup-controller.ts";
import { describeLaunchState, describeWorkspaceList } from "./application/setup.ts";
import { createTauriTransport } from "./application/tauri.ts";
import appIcon from "./assets/app-icon.svg";

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

export function App() {
  const [projectsOpen, setProjectsOpen] = useState(true);
  const [detailsOpen, setDetailsOpen] = useState(true);
  const controller = useMemo(
    () => createSetupController(createApplicationBridge(createTauriTransport())),
    [],
  );
  const state = useSyncExternalStore(controller.subscribe, controller.getState);

  useEffect(() => {
    void controller.initialize();
    return () => controller.dispose();
  }, [controller]);

  const launch = describeLaunchState({
    setup: state.setup,
    runtimeState: state.runtimeState,
    failure: state.failure,
  });
  const recent = describeWorkspaceList(state.recentWorkspaces);
  const agent = state.capabilities?.agent;
  const authMethods = state.capabilities?.authenticationMethods ?? [];
  const cockpitClassName = [
    "cockpit",
    !projectsOpen && "cockpit--projects-collapsed",
    !detailsOpen && "cockpit--details-collapsed",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className="app-shell">
      <header className="titlebar">
        <div className="brand">
          <img className="brand__mark" src={appIcon} alt="" />
          <div>
            <p className="brand__name">Grok Build GUI</p>
            <p className="brand__tagline">Local agent cockpit</p>
          </div>
        </div>
        <div className="titlebar__actions">
          <div className="panel-toggles" aria-label="Panel visibility">
            <button
              type="button"
              aria-controls="workspace-panel"
              aria-expanded={projectsOpen}
              onClick={() => setProjectsOpen((open) => !open)}
            >
              Projects
            </button>
            <button
              type="button"
              aria-controls="details-panel"
              aria-expanded={detailsOpen}
              onClick={() => setDetailsOpen((open) => !open)}
            >
              Details
            </button>
          </div>
          <div
            className={`runtime-pill runtime-pill--${launch.kind}`}
            aria-label={`Runtime status: ${launch.label}`}
            aria-live="polite"
          >
            <span className="runtime-pill__dot" aria-hidden="true" />
            {launch.label}
          </div>
        </div>
      </header>

      <main
        className={cockpitClassName}
        aria-label="Grok Build setup"
      >
        {projectsOpen ? (
          <aside
            id="workspace-panel"
            className="shell-panel workspace-panel"
            aria-labelledby="workspace-heading"
          >
          <div className="panel-heading">
            <div>
              <p className="shell-panel__eyebrow">Workspace</p>
              <h1 id="workspace-heading">Projects</h1>
            </div>
            <span className="count-badge" aria-label={`${state.recentWorkspaces.length} recent`}>
              {state.recentWorkspaces.length}
            </span>
          </div>

          <button
            className="button button--primary button--wide"
            type="button"
            disabled={!state.setup?.runtimeAvailable || state.choosingWorkspace}
            onClick={() => void controller.pickWorkspace().catch(() => undefined)}
          >
            <span aria-hidden="true">＋</span>
            {state.choosingWorkspace ? "Opening picker…" : "Choose workspace"}
          </button>

          <div className="recent-heading">
            <span>Recent</span>
            {recent.staleCount > 0 ? <span>{recent.staleCount} unavailable</span> : null}
          </div>

          {recent.kind === "empty" ? (
            <div className="empty-list">
              <span className="empty-list__icon" aria-hidden="true">⌁</span>
              <p>No recent workspaces</p>
              <span>Choose a folder to keep it close.</span>
            </div>
          ) : (
            <ul className="workspace-list">
              {state.recentWorkspaces.map((workspace) => (
                <li key={workspace.path} className={!workspace.available ? "is-stale" : undefined}>
                  <button
                    className="workspace-list__open"
                    type="button"
                    disabled={!workspace.available}
                    title={workspace.path}
                    onClick={() =>
                      void controller.openRecent(workspace.path).catch(() => undefined)
                    }
                  >
                    <span className="workspace-list__mark" aria-hidden="true">▰</span>
                    <span>
                      <strong>{workspaceName(workspace.path)}</strong>
                      <small>{workspace.available ? workspace.path : "Folder unavailable"}</small>
                    </span>
                  </button>
                  <button
                    className="workspace-list__remove"
                    type="button"
                    aria-label={`Remove ${workspaceName(workspace.path)} from recent workspaces`}
                    onClick={() =>
                      void controller.removeRecent(workspace.path).catch(() => undefined)
                    }
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          )}
          </aside>
        ) : null}

        <section className="shell-panel shell-panel--main" aria-labelledby="launch-heading">
          <div className={`launch-card launch-card--${launch.kind}`}>
            <div className="launch-card__icon" aria-hidden="true">
              <img src={appIcon} alt="" />
              <span />
            </div>
            <p className="shell-panel__eyebrow">Local setup</p>
            <h1 id="launch-heading">{launch.heading}</h1>
            <p>{launch.detail}</p>

            {launch.canRetry ? (
              <button
                className="button button--primary"
                type="button"
                onClick={() => void controller.retry()}
              >
                Reconnect
              </button>
            ) : null}

            {state.selectedWorkspace !== null ? (
              <div className="selected-workspace" aria-label="Selected workspace">
                <span className="selected-workspace__check" aria-hidden="true">✓</span>
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
        </section>

        {detailsOpen ? (
          <aside
            id="details-panel"
            className="shell-panel details-panel"
            aria-labelledby="details-heading"
          >
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
              <dd>{state.capabilities ? `ACP v${state.capabilities.protocolVersion}` : "—"}</dd>
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
          </aside>
        ) : null}
      </main>

      <footer className="statusbar">
        <span>Local assets only</span>
        <span aria-hidden="true">·</span>
        <span>Native folder picker through a reviewed Rust command</span>
        <span aria-hidden="true">·</span>
        <span>No credential-file access</span>
      </footer>
    </div>
  );
}

function workspaceName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const separator = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  return trimmed.slice(separator + 1) || trimmed;
}
