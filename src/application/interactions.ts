import type { ElicitationControl, ElicitationDecision, ElicitationValue, PermissionDecision, PermissionScope } from "./contract.ts";
import type { ApplicationState, ElicitationRequestState, PermissionRequestState } from "./state.ts";

export const MAX_INPUT_BYTES = 16 * 1024;
const MAX_SCOPE_BYTES = 8 * 1024;
const encoder = new TextEncoder();
const controls = /[\u0000-\u001f\u007f-\u009f]/u;

export type InteractionContext = {
  application: ApplicationState;
  sessionId: string | null;
  epoch: number;
  blocked: boolean;
};

type Identity = { key: string; generation: number; epoch: number; sessionId: string | null };
export type Interaction = Identity & (
  | { kind: "permission"; request: PermissionRequestState }
  | { kind: "elicitation"; request: ElicitationRequestState }
);

export function responseKey(target: Interaction): string {
  return JSON.stringify([target.generation, target.kind, target.request.id]);
}

export function pendingInteractions(context: InteractionContext): Interaction[] {
  const { application, sessionId, epoch, blocked } = context;
  if (blocked || application.needsResync || application.pendingEventCount > 0 ||
      !["ready", "working", "waiting_for_input"].includes(application.runtime.state)) return [];
  const generation = application.generation;
  const identity = (id: string, kind: string, owner: string | null): Identity => ({
    key: JSON.stringify([generation, epoch, owner, kind, id]), generation, epoch, sessionId: owner,
  });
  const result: Interaction[] = Object.values(application.runtime.elicitations).map(request => ({
    ...identity(request.id, "elicitation", null), kind: "elicitation", request,
  }));
  const session = sessionId === null ? undefined : application.sessions[sessionId];
  if (session === undefined || !["ready", "working", "waiting_for_input"].includes(session.state)) return result;
  for (const request of Object.values(session.permissions)) {
    result.push({ ...identity(request.id, "permission", sessionId), kind: "permission", request });
  }
  for (const request of Object.values(session.elicitations)) {
    result.push({ ...identity(request.id, "elicitation", sessionId), kind: "elicitation", request });
  }
  return result;
}

function exactText(value: unknown, limit: number, empty = false): value is string {
  return typeof value === "string" && (empty || value.trim().length > 0) &&
    encoder.encode(value).length <= limit && !controls.test(value);
}

function absolutePath(value: unknown): value is string {
  return exactText(value, MAX_SCOPE_BYTES) && /^(\/|[a-z]:[\\/]|\\\\[^\\]+\\[^\\]+)/iu.test(value);
}

export function validPermissionScope(scope: PermissionScope): boolean {
  if (!scope || typeof scope !== "object") return false;
  switch (scope.type) {
    case "command": return exactText(scope.command, MAX_SCOPE_BYTES) && absolutePath(scope.workingDirectory) &&
      Array.isArray(scope.affectedPaths) && scope.affectedPaths.length <= 64 && scope.affectedPaths.every(absolutePath);
    case "filesystem": return ["read", "edit", "delete", "move"].includes(scope.operation) && absolutePath(scope.path);
    // These shapes do not yet describe enough scope to approve safely.
    default: return false;
  }
}

const decisionLabels: Record<PermissionDecision, string> = {
  deny_once: "Deny once", deny_always: "Always deny", cancel: "Cancel request",
  allow_once: "Allow once", allow_always: "Always allow",
};

export function permissionConsequenceTone(
  request: PermissionRequestState,
): "destructive" | "review" {
  const scope = request.scope;
  const text = `${request.title} ${request.consequence ?? ""} ${scope.type === "command" ? scope.command : ""}`;
  if (scope.type === "filesystem" && (scope.operation === "delete" || scope.operation === "move")) {
    return "destructive";
  }
  if (/(?:\brm\b|\bdel\b|\bremove\b|\bdelete\b|overwrit)/iu.test(text)) {
    return "destructive";
  }
  return "review";
}

export function permissionActions(request: PermissionRequestState) {
  const decisions = request.availableDecisions;
  const valid = Array.isArray(decisions) && decisions.length > 0 && decisions.length <= 5 &&
    new Set(decisions).size === decisions.length && decisions.every(value => Object.hasOwn(decisionLabels, value));
  const approvable = valid && validPermissionScope(request.scope);
  const advertised = valid ? decisions : [];
  // Cancellation is a protocol-independent application intent, always supported by the bridge.
  return (["deny_once", "deny_always", "cancel", "allow_once", "allow_always"] as const)
    .filter(decision => decision === "cancel" || (advertised.includes(decision) &&
      (approvable || !decision.startsWith("allow"))))
    .map(decision => ({ decision, label: decisionLabels[decision], persistent: decision.endsWith("always") }));
}

export function validElicitationControl(control: ElicitationControl): boolean {
  if (!control || control.type === "other" || !exactText(control.fieldId, 256)) return false;
  switch (control.type) {
    case "text": return typeof control.sensitive === "boolean" && Number.isSafeInteger(control.minLength) &&
      Number.isSafeInteger(control.maxLength) && control.minLength >= 0 &&
      control.maxLength >= control.minLength && control.maxLength <= MAX_INPUT_BYTES;
    case "confirmation": return true;
    case "choice": return control.truncated === false && typeof control.multiple === "boolean" &&
      Array.isArray(control.options) && control.options.length > 0 && control.options.length <= 64 &&
      new Set(control.options).size === control.options.length && control.options.every(value => exactText(value, 512));
    default: return false;
  }
}

export type FormInput = string | boolean | string[] | null;
export type FormResult = { decision: ElicitationDecision; error: null } | { decision: null; error: string };

export function elicitationAnswer(control: ElicitationControl, input: FormInput): FormResult {
  const invalid = (error: string): FormResult => ({ decision: null, error });
  if (!validElicitationControl(control) || control.type === "other") return invalid("This form is unsupported. Cancel the request.");
  let value: ElicitationValue;
  switch (control.type) {
    case "text": {
      if (!exactText(input, MAX_INPUT_BYTES, true)) return invalid("Use plain text within the 16 KB limit, without control characters.");
      const length = Array.from(input).length;
      if (length < control.minLength || length > control.maxLength) return invalid(`Enter ${control.minLength}–${control.maxLength} characters.`);
      value = { type: "string", value: input };
      break;
    }
    case "confirmation":
      if (typeof input !== "boolean") return invalid("Choose Yes or No.");
      value = { type: "boolean", value: input };
      break;
    case "choice":
      if (control.multiple) {
        if (!Array.isArray(input) || input.length === 0 || input.length > 64 || new Set(input).size !== input.length ||
            !input.every(item => control.options.includes(item))) return invalid("Choose one or more of the listed options.");
        value = { type: "string_array", value: [...input] };
      } else {
        if (typeof input !== "string" || !control.options.includes(input)) return invalid("Choose a listed option.");
        value = { type: "string", value: input };
      }
      break;
  }
  return { decision: { type: "accept", value: { content: { [control.fieldId]: value } } }, error: null };
}
