import { useState } from "react";

import type { CompatibilityReport } from "../application/compatibility.ts";
import { copyText, serializeCompatibilityReport } from "../application/compatibility.ts";
import { Dialog } from "./Dialog.tsx";

export function CompatibilityDialog({
  report,
  onClose,
}: {
  report: CompatibilityReport;
  onClose: () => void;
}) {
  const text = serializeCompatibilityReport(report);
  const [copyState, setCopyState] = useState<"idle" | "copied" | "unavailable">("idle");

  return (
    <Dialog
      title="Compatibility report"
      onClose={onClose}
      footer={
        <button
          className="button button--primary"
          type="button"
          onClick={() => {
            void copyText(text).then((result) => setCopyState(result));
          }}
        >
          Copy privacy-safe report
        </button>
      }
    >
      <p>
        Generated from negotiated runtime data. It excludes credentials, private paths, session IDs,
        prompt text, stderr, and workspace contents.
      </p>
      <dl className="detail-list">
        <div>
          <dt>App</dt>
          <dd>
            {report.app.name} {report.app.version}
          </dd>
        </div>
        <div>
          <dt>Runtime</dt>
          <dd>{report.runtime.state}</dd>
        </div>
        <div>
          <dt>Agent</dt>
          <dd>
            {report.runtime.agentProduct}
            {report.runtime.agentVersion !== null ? ` ${report.runtime.agentVersion}` : ""}
          </dd>
        </div>
        <div>
          <dt>Protocol</dt>
          <dd>
            {report.runtime.protocolVersion === null
              ? "Not reported"
              : `ACP v${report.runtime.protocolVersion}`}
          </dd>
        </div>
        <div>
          <dt>Grok executable</dt>
          <dd>{report.runtime.executableState}</dd>
        </div>
      </dl>
      <ul className="feature-list">
        {report.features.map((feature) => (
          <li key={feature.id}>
            <strong>
              {feature.label}: {statusLabel(feature.status)}
            </strong>
            <span>{feature.note}</span>
          </li>
        ))}
      </ul>
      <label className="report-copy">
        Privacy-safe JSON
        <textarea readOnly value={text} spellCheck={false} />
      </label>
      {copyState === "copied" ? <p role="status">Copied to the clipboard.</p> : null}
      {copyState === "unavailable" ? (
        <p role="status">Clipboard is unavailable. Select the JSON and copy it manually.</p>
      ) : null}
    </Dialog>
  );
}

function statusLabel(status: CompatibilityReport["features"][number]["status"]): string {
  if (status === "advertised") {
    return "Advertised";
  }
  if (status === "not_advertised") {
    return "Not advertised";
  }
  return "Not reported";
}
