import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles.css";

const mount = document.getElementById("root");

if (mount === null) {
  throw new Error("The application root element is missing.");
}

const root = mount;

async function render() {
  const useConversationFixture =
    import.meta.env.DEV &&
    new URLSearchParams(window.location.search).get("fixture") === "conversation";

  if (useConversationFixture) {
    const { FIXTURE_WORKSPACE, createConversationFixtureTransport } = await import(
      "./application/conversation-fixture.ts"
    );
    createRoot(root).render(
      <StrictMode>
        <App
          transport={createConversationFixtureTransport()}
          autoWorkspace={FIXTURE_WORKSPACE}
          autoOpenFirstSession
        />
      </StrictMode>,
    );
    return;
  }

  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void render();
