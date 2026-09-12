export type ShortcutId =
  | "toggle-projects"
  | "toggle-details"
  | "focus-projects"
  | "focus-conversation"
  | "focus-composer"
  | "new-session"
  | "send-prompt"
  | "cancel-or-dismiss"
  | "appearance"
  | "shortcuts"
  | "onboarding"
  | "compatibility";

export type ShortcutDefinition = {
  id: ShortcutId;
  keys: string;
  group: "Panels" | "Sessions" | "Conversation" | "Help";
  label: string;
};

export const KEYBOARD_SHORTCUTS: ShortcutDefinition[] = [
  {
    id: "toggle-projects",
    keys: "Ctrl+1",
    group: "Panels",
    label: "Show or hide the projects panel",
  },
  {
    id: "toggle-details",
    keys: "Ctrl+2",
    group: "Panels",
    label: "Show or hide the details panel",
  },
  {
    id: "focus-projects",
    keys: "Alt+1",
    group: "Panels",
    label: "Move focus to projects",
  },
  {
    id: "focus-conversation",
    keys: "Alt+2",
    group: "Panels",
    label: "Move focus to conversation",
  },
  {
    id: "focus-composer",
    keys: "Ctrl+L",
    group: "Conversation",
    label: "Focus the prompt",
  },
  {
    id: "new-session",
    keys: "Ctrl+N",
    group: "Sessions",
    label: "Start a new session",
  },
  {
    id: "send-prompt",
    keys: "Ctrl+Enter",
    group: "Conversation",
    label: "Send the prompt",
  },
  {
    id: "cancel-or-dismiss",
    keys: "Escape",
    group: "Conversation",
    label: "Close a dialog or cancel the active turn",
  },
  {
    id: "appearance",
    keys: "Ctrl+,",
    group: "Help",
    label: "Appearance and motion",
  },
  {
    id: "shortcuts",
    keys: "Ctrl+/",
    group: "Help",
    label: "Keyboard shortcuts",
  },
  {
    id: "onboarding",
    keys: "Ctrl+Shift+O",
    group: "Help",
    label: "Reopen the setup guide",
  },
  {
    id: "compatibility",
    keys: "Ctrl+Shift+R",
    group: "Help",
    label: "Show the compatibility report",
  },
];

export type ShortcutEvent = {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
};

export function matchShortcut(event: ShortcutEvent): ShortcutId | null {
  const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
  const modifier = event.ctrlKey || event.metaKey;

  if (key === "Escape") {
    return "cancel-or-dismiss";
  }
  if (event.altKey && !modifier && key === "1") {
    return "focus-projects";
  }
  if (event.altKey && !modifier && key === "2") {
    return "focus-conversation";
  }
  if (!modifier) {
    return null;
  }
  if (event.shiftKey && key === "o") {
    return "onboarding";
  }
  if (event.shiftKey && key === "r") {
    return "compatibility";
  }
  if (event.shiftKey) {
    return null;
  }
  if (key === "1") {
    return "toggle-projects";
  }
  if (key === "2") {
    return "toggle-details";
  }
  if (key === "l") {
    return "focus-composer";
  }
  if (key === "n") {
    return "new-session";
  }
  if (key === "Enter") {
    return "send-prompt";
  }
  if (key === "," || event.key === "Comma") {
    return "appearance";
  }
  if (key === "/" || key === "?" || event.key === "Slash") {
    return "shortcuts";
  }
  return null;
}

export function shortcutConsumesEvent(
  id: ShortcutId,
  target: { tagName?: string; isContentEditable?: boolean } | null,
): boolean {
  const tag = target?.tagName?.toLowerCase();
  const editable = tag === "input" || tag === "textarea" || tag === "select" ||
    target?.isContentEditable === true;
  if (!editable) {
    return true;
  }
  return id === "cancel-or-dismiss" || id === "send-prompt" || id === "toggle-projects" ||
    id === "toggle-details" || id === "appearance" || id === "shortcuts" ||
    id === "onboarding" || id === "compatibility";
}
