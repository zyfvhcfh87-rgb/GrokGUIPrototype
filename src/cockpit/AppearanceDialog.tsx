import type { PresentationMotion, PresentationTheme } from "../application/contract.ts";
import type { PresentationPreferences } from "../application/presentation.ts";
import { Dialog } from "./Dialog.tsx";

export function AppearanceDialog({
  preferences,
  onChange,
  onClose,
}: {
  preferences: PresentationPreferences;
  onChange: (patch: Partial<PresentationPreferences>) => void;
  onClose: () => void;
}) {
  return (
    <Dialog title="Appearance" onClose={onClose}>
      <fieldset className="appearance-fieldset">
        <legend>Theme</legend>
        {(["system", "light", "dark"] as const).map((theme) => (
          <label key={theme}>
            <input
              type="radio"
              name="theme"
              checked={preferences.theme === theme}
              onChange={() => onChange({ theme })}
            />
            {themeLabel(theme)}
          </label>
        ))}
      </fieldset>
      <fieldset className="appearance-fieldset">
        <legend>Motion</legend>
        {(["system", "reduce"] as const).map((motion) => (
          <label key={motion}>
            <input
              type="radio"
              name="motion"
              checked={preferences.motion === motion}
              onChange={() => onChange({ motion })}
            />
            {motionLabel(motion)}
          </label>
        ))}
      </fieldset>
      <p>
        High contrast follows the operating system. Side panels stay resizable and can collapse
        without shrinking the conversation below a comfortable width.
      </p>
    </Dialog>
  );
}

function themeLabel(theme: PresentationTheme): string {
  if (theme === "system") {
    return "Match the operating system";
  }
  return theme === "light" ? "Light" : "Dark";
}

function motionLabel(motion: PresentationMotion): string {
  return motion === "reduce" ? "Reduce motion" : "Match the operating system";
}
