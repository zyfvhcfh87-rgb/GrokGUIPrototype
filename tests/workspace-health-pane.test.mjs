import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { describeRuntimeHealth, describeWorkspaceChanges } from "../src/application/workspace-health.ts";

let React;
let renderToStaticMarkup;
let WorkspaceHealthPane;
let unavailable = false;
try {
  React = await import("react");
  ({ renderToStaticMarkup } = await import("react-dom/server"));
} catch (error) {
  if (error.code !== "ERR_MODULE_NOT_FOUND") {
    throw error;
  }
  unavailable = "React dependencies are unavailable; run npm ci before component tests.";
}

if (!unavailable) {
  try {
    const { transformWithOxc } = await import("vite");
    const file = new URL("../src/cockpit/WorkspaceHealthPane.tsx", import.meta.url);
    const source = await readFile(file, "utf8");
    const output = (
      await transformWithOxc(source, file.pathname, {
        jsx: { runtime: "automatic" },
      })
    ).code.replace(/from "([^"]+)"/gu, (_, specifier) =>
      `from ${JSON.stringify(specifier.startsWith(".") ? new URL(specifier, file).href : import.meta.resolve(specifier))}`,
    );
    ({ WorkspaceHealthPane } = await import(
      `data:text/javascript;base64,${Buffer.from(output).toString("base64")}`
    ));
  } catch (error) {
    if (error.code !== "ERR_MODULE_NOT_FOUND") {
      throw error;
    }
    unavailable = "Vite is unavailable; run npm ci before component tests.";
  }
}

test("workspace health pane is read-only and shows the attribution disclaimer", { skip: unavailable }, () => {
  const html = renderToStaticMarkup(
    React.createElement(WorkspaceHealthPane, {
      changes: describeWorkspaceChanges({
        workspace: "C:\\work",
        changes: {
          kind: "repository",
          attributableToSession: false,
          entries: [
            {
              path: "src/lib.rs",
              previousPath: null,
              status: "modified",
              content: "text",
              diff: "@@ -1 +1 @@\n+safe",
              truncated: false,
            },
          ],
          truncated: false,
          omittedEntryCount: 0,
          omittedLineCount: 0,
        },
        failure: null,
        loading: false,
      }),
      health: describeRuntimeHealth({
        diagnostics: {
          state: "failed",
          workerRunning: false,
          consecutiveFailures: 2,
          lastFailure: {
            code: "connection_failed",
            diagnostic: "runtime process ended",
            recoverable: true,
          },
          stderrLines: 1,
          stderrBytes: 12,
          stderrTruncatedLines: 0,
          stderrReadErrors: 0,
          processContainment: "direct_child",
        },
        failure: null,
        loading: false,
      }),
      onRefreshChanges() {},
      onRefreshDiagnostics() {},
      onRecover() {},
    }),
  );

  assert.match(html, /not automatically attributable/);
  assert.match(html, /src\/lib.rs/);
  assert.match(html, /Recover/);
  assert.doesNotMatch(html, /stage|commit|discard|delete workspace/iu);
});
