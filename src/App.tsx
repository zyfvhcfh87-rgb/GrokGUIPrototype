import appIcon from "./assets/app-icon.svg";

const shellAreas = [
  {
    eyebrow: "Workspace",
    title: "Projects & sessions",
    body: "Your recent workspaces and Grok-owned sessions will live here.",
  },
  {
    eyebrow: "Conversation",
    title: "A calm place to build",
    body: "The streamed conversation and composer arrive with the vertical slice.",
  },
  {
    eyebrow: "Context",
    title: "Plan & activity",
    body: "Plans, tool activity, and read-only workspace changes stay visible here.",
  },
] as const;

export function App() {
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
        <div className="runtime-pill" aria-label="Runtime status: shell ready">
          <span className="runtime-pill__dot" aria-hidden="true" />
          Shell ready
        </div>
      </header>

      <main className="cockpit" aria-label="Grok Build workspace">
        {shellAreas.map((area, index) => (
          <section
            className={`shell-panel shell-panel--${index + 1}`}
            key={area.title}
            aria-labelledby={`panel-title-${index}`}
          >
            <p className="shell-panel__eyebrow">{area.eyebrow}</p>
            <h1 id={`panel-title-${index}`}>{area.title}</h1>
            <p className="shell-panel__body">{area.body}</p>
            {index === 1 ? (
              <div className="empty-conversation" aria-label="Conversation not started">
                <img src={appIcon} alt="" />
                <p>Secure shell online</p>
                <span>Runtime wiring is the next layer.</span>
              </div>
            ) : (
              <div className="placeholder-stack" aria-hidden="true">
                <span />
                <span />
                <span />
              </div>
            )}
          </section>
        ))}
      </main>

      <footer className="statusbar">
        <span>Local assets only</span>
        <span aria-hidden="true">·</span>
        <span>No filesystem, shell, dialog, or remote-link access</span>
      </footer>
    </div>
  );
}
