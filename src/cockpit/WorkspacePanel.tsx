import { workspaceName, shortSessionId } from "../application/display.ts";
import type { SessionListPresentation, SessionRow } from "../application/sessions.ts";
import { describeSessionStatus } from "../application/sessions.ts";
import type { RecentWorkspace, Workspace } from "../application/contract.ts";

export function WorkspacePanel({
  recentKind,
  staleCount,
  recentWorkspaces,
  choosingWorkspace,
  runtimeAvailable,
  selectedWorkspace,
  sessionList,
  sessions,
  sessionFocus,
  creating,
  opening,
  closing,
  loadingMore,
  onPickWorkspace,
  onOpenRecent,
  onRemoveRecent,
  onCreateSession,
  onOpenSession,
  onCloseSession,
  onRetrySessions,
  onLoadMore,
  onSessionFocus,
}: {
  recentKind: "empty" | "stale" | "ready" | "mixed";
  staleCount: number;
  recentWorkspaces: RecentWorkspace[];
  choosingWorkspace: boolean;
  runtimeAvailable: boolean;
  selectedWorkspace: Workspace | null;
  sessionList: SessionListPresentation;
  sessions: SessionRow[];
  sessionFocus: number;
  creating: boolean;
  opening: boolean;
  closing: boolean;
  loadingMore: boolean;
  onPickWorkspace: () => void;
  onOpenRecent: (path: string) => void;
  onRemoveRecent: (path: string) => void;
  onCreateSession: () => void;
  onOpenSession: (sessionId: string) => void;
  onCloseSession: (sessionId: string) => void;
  onRetrySessions: () => void;
  onLoadMore: () => void;
  onSessionFocus: (index: number) => void;
}) {
  return (
    <aside
      id="workspace-panel"
      className="shell-panel workspace-panel"
      aria-labelledby="workspace-heading"
    >
      <div className="panel-heading">
        <div>
          <p className="shell-panel__eyebrow">Workspace</p>
          <h1 id="workspace-heading" tabIndex={-1}>
            Projects
          </h1>
        </div>
        <span className="count-badge" aria-label={`${recentWorkspaces.length} recent`}>
          {recentWorkspaces.length}
        </span>
      </div>

      <button
        className="button button--primary button--wide"
        type="button"
        disabled={!runtimeAvailable || choosingWorkspace}
        onClick={onPickWorkspace}
      >
        <span aria-hidden="true">＋</span>
        {choosingWorkspace ? "Opening picker…" : "Choose workspace"}
      </button>

      <div className="recent-heading">
        <span>Recent</span>
        {staleCount > 0 ? <span>{staleCount} unavailable</span> : null}
      </div>

      {recentKind === "empty" ? (
        <div className="empty-list">
          <span className="empty-list__icon" aria-hidden="true">
            ⌁
          </span>
          <p>No recent workspaces</p>
          <span>Choose a folder to keep it close.</span>
        </div>
      ) : (
        <ul className="workspace-list">
          {recentWorkspaces.map((workspace) => (
            <li key={workspace.path} className={!workspace.available ? "is-stale" : undefined}>
              <button
                className="workspace-list__open"
                type="button"
                disabled={!workspace.available}
                title={workspace.path}
                onClick={() => onOpenRecent(workspace.path)}
              >
                <span className="workspace-list__mark" aria-hidden="true">
                  ▰
                </span>
                <span>
                  <strong>{workspaceName(workspace.path)}</strong>
                  <small>{workspace.available ? workspace.path : "Folder unavailable"}</small>
                </span>
              </button>
              <button
                className="workspace-list__remove"
                type="button"
                aria-label={`Remove ${workspaceName(workspace.path)} from recent workspaces`}
                onClick={() => onRemoveRecent(workspace.path)}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}

      {selectedWorkspace !== null ? (
        <section className="session-section" aria-labelledby="session-heading">
          <div className="recent-heading">
            <span id="session-heading">Sessions</span>
            <span>{sessions.length}</span>
          </div>
          <button
            className="button button--wide"
            type="button"
            disabled={!sessionList.canCreate || creating}
            onClick={onCreateSession}
          >
            {creating ? "Creating session…" : "New session"}
          </button>
          {sessionList.kind === "loading" ||
          sessionList.kind === "empty" ||
          sessionList.kind === "failed" ||
          sessionList.kind === "stale" ||
          sessionList.kind === "unavailable" ? (
            <div className={`empty-list empty-list--${sessionList.kind}`}>
              <span className="empty-list__icon" aria-hidden="true">
                {sessionList.kind === "failed" ? "!" : "◌"}
              </span>
              <p>{sessionList.heading}</p>
              <span>{sessionList.detail}</span>
              {sessionList.canRetry ? (
                <button className="button" type="button" onClick={onRetrySessions}>
                  Retry
                </button>
              ) : null}
            </div>
          ) : null}
          {sessions.length > 0 ? (
            <ul
              className="session-list"
              role="listbox"
              aria-label="Sessions in the selected workspace"
              aria-activedescendant={
                sessions[sessionFocus]?.sessionId
                  ? `session-${sessions[sessionFocus].sessionId}`
                  : undefined
              }
            >
              {sessions.map((item, index) => (
                <li
                  key={item.sessionId}
                  id={`session-${item.sessionId}`}
                  className={[
                    item.status === "selected" && "is-selected",
                    item.status === "closed" && "is-closed",
                    item.status === "failed" && "is-failed",
                    item.status === "stale" && "is-stale",
                  ]
                    .filter(Boolean)
                    .join(" ") || undefined}
                  role="option"
                  aria-selected={item.status === "selected"}
                >
                  <button
                    className="session-list__open"
                    type="button"
                    disabled={!sessionList.canOpen || opening}
                    tabIndex={index === sessionFocus ? 0 : -1}
                    onFocus={() => onSessionFocus(index)}
                    onClick={() => onOpenSession(item.sessionId)}
                    onKeyDown={(event) => {
                      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                        event.preventDefault();
                        const next =
                          event.key === "ArrowDown"
                            ? Math.min(index + 1, sessions.length - 1)
                            : Math.max(index - 1, 0);
                        onSessionFocus(next);
                        document
                          .getElementById(`session-open-${sessions[next]?.sessionId}`)
                          ?.focus();
                      }
                      if (event.key === "Home") {
                        event.preventDefault();
                        onSessionFocus(0);
                        document.getElementById(`session-open-${sessions[0]?.sessionId}`)?.focus();
                      }
                      if (event.key === "End") {
                        event.preventDefault();
                        const last = sessions.length - 1;
                        onSessionFocus(last);
                        document
                          .getElementById(`session-open-${sessions[last]?.sessionId}`)
                          ?.focus();
                      }
                      if (
                        (event.key === "Delete" || event.key === "Backspace") &&
                        sessionList.canClose
                      ) {
                        event.preventDefault();
                        onCloseSession(item.sessionId);
                      }
                    }}
                    id={`session-open-${item.sessionId}`}
                  >
                    <span className="workspace-list__mark" aria-hidden="true">
                      ▹
                    </span>
                    <span>
                      <strong>{item.title ?? shortSessionId(item.sessionId)}</strong>
                      <small>{describeSessionStatus(item.status)}</small>
                    </span>
                  </button>
                  <button
                    className="workspace-list__remove"
                    type="button"
                    disabled={!sessionList.canClose || closing}
                    aria-label={`Close session ${item.title ?? shortSessionId(item.sessionId)}`}
                    onClick={() => onCloseSession(item.sessionId)}
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
          {sessionList.canLoadMore ? (
            <button
              className="button button--wide"
              type="button"
              disabled={loadingMore}
              onClick={onLoadMore}
            >
              {loadingMore ? "Loading more…" : "Load more"}
            </button>
          ) : null}
        </section>
      ) : null}
    </aside>
  );
}
