import type { LaunchPresentation } from "../application/setup.ts";

export function Titlebar({
  launch,
  brandMark,
  projectsOpen,
  detailsOpen,
  onToggleProjects,
  onToggleDetails,
  onOpenAppearance,
  onOpenShortcuts,
  onOpenOnboarding,
  onOpenCompatibility,
}: {
  launch: LaunchPresentation;
  brandMark: string;
  projectsOpen: boolean;
  detailsOpen: boolean;
  onToggleProjects: () => void;
  onToggleDetails: () => void;
  onOpenAppearance: () => void;
  onOpenShortcuts: () => void;
  onOpenOnboarding: () => void;
  onOpenCompatibility: () => void;
}) {
  return (
    <header className="titlebar">
      <div className="brand">
        <img className="brand__mark" src={brandMark} alt="" />
        <div>
          <p className="brand__name">Grok Build GUI</p>
          <p className="brand__tagline">Local agent cockpit</p>
        </div>
      </div>
      <div className="titlebar__actions">
        <div className="panel-toggles" aria-label="Panel visibility">
          <button
            id="toggle-projects"
            type="button"
            aria-controls="workspace-panel"
            aria-expanded={projectsOpen}
            onClick={onToggleProjects}
          >
            Projects
          </button>
          <button
            id="toggle-details"
            type="button"
            aria-controls="details-panel"
            aria-expanded={detailsOpen}
            onClick={onToggleDetails}
          >
            Details
          </button>
        </div>
        <button className="button" type="button" onClick={onOpenAppearance}>
          Appearance
        </button>
        <button className="button" type="button" onClick={onOpenShortcuts}>
          Shortcuts
        </button>
        <button className="button" type="button" onClick={onOpenOnboarding}>
          Setup guide
        </button>
        <button className="button" type="button" onClick={onOpenCompatibility}>
          Compatibility
        </button>
        <div
          className={`runtime-pill runtime-pill--${launch.kind}`}
          aria-label={`Runtime status: ${launch.label}`}
        >
          <span className="runtime-pill__dot" aria-hidden="true" />
          <span>{launch.label}</span>
        </div>
      </div>
    </header>
  );
}
