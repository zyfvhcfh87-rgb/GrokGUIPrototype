import type { RuntimeHealthPresentation, WorkspaceChangesPresentation } from "../application/workspace-health.ts";
import { describeChangeContent, describeChangeStatus } from "../application/workspace-health.ts";

export function WorkspaceHealthPane({
  changes,
  health,
  onRefreshChanges,
  onRefreshDiagnostics,
  onRecover,
}: {
  changes: WorkspaceChangesPresentation;
  health: RuntimeHealthPresentation;
  onRefreshChanges: () => void;
  onRefreshDiagnostics: () => void;
  onRecover: () => void;
}) {
  return (
    <div className="workspace-health">
      <section className="workspace-health__section" aria-labelledby="workspace-changes-heading">
        <div className="recent-heading">
          <h2 id="workspace-changes-heading">{changes.heading}</h2>
          {changes.canRefresh ? (
            <button className="button" type="button" onClick={onRefreshChanges}>
              Refresh
            </button>
          ) : null}
        </div>
        <p>{changes.detail}</p>
        <p className="workspace-health__disclaimer">{changes.disclaimer}</p>
        {changes.truncated ? (
          <p className="workspace-health__muted">
            The view is bounded
            {changes.omittedEntryCount > 0 ? ` · ${changes.omittedEntryCount} paths omitted` : ""}
            {changes.omittedLineCount > 0 ? ` · ${changes.omittedLineCount} diff lines omitted` : ""}
            .
          </p>
        ) : null}
        {changes.entries.length > 0 ? (
          <ul className="change-list" aria-label="Workspace changes">
            {changes.entries.map((entry) => (
              <li key={`${entry.status}:${entry.path}`}>
                <details>
                  <summary>
                    <strong>{entry.path}</strong>
                    <small>
                      {describeChangeStatus(entry.status)} · {describeChangeContent(entry.content)}
                    </small>
                  </summary>
                  {entry.previousPath !== null ? <p>Previously {entry.previousPath}</p> : null}
                  {entry.diff !== null ? (
                    <pre className="change-diff">
                      {entry.diff}
                      {entry.truncated ? "\n[truncated]" : ""}
                    </pre>
                  ) : (
                    <p className="workspace-health__muted">
                      {entry.content === "binary"
                        ? "Binary content is not shown."
                        : entry.content === "omitted"
                          ? "File contents are omitted from this read-only view."
                          : "No diff is available."}
                    </p>
                  )}
                </details>
              </li>
            ))}
          </ul>
        ) : null}
      </section>

      <section className="workspace-health__section" aria-labelledby="runtime-health-heading">
        <div className="recent-heading">
          <h2 id="runtime-health-heading">{health.heading}</h2>
          <button className="button" type="button" onClick={onRefreshDiagnostics}>
            Refresh
          </button>
        </div>
        <p>{health.detail}</p>
        <dl className="detail-list">
          <div>
            <dt>Child process</dt>
            <dd>{health.workerRunning ? "Running" : "Stopped"}</dd>
          </div>
          <div>
            <dt>Containment</dt>
            <dd>{health.containmentLabel}</dd>
          </div>
          <div>
            <dt>Stderr lines</dt>
            <dd>{health.stderrLines}</dd>
          </div>
          <div>
            <dt>Stderr bytes</dt>
            <dd>{health.stderrBytes}</dd>
          </div>
          {health.consecutiveFailures > 0 ? (
            <div>
              <dt>Consecutive failures</dt>
              <dd>{health.consecutiveFailures}</dd>
            </div>
          ) : null}
        </dl>
        {health.lastFailure !== null ? (
          <div className="inline-alert" role="alert">
            <strong>Last diagnostic</strong>
            <span>{health.lastFailure.diagnostic}</span>
          </div>
        ) : null}
        {health.canRecover ? (
          <button className="button button--primary" type="button" onClick={onRecover}>
            Recover
          </button>
        ) : null}
      </section>
    </div>
  );
}
