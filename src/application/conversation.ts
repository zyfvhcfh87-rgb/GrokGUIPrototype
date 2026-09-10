import type {
  ActivityStatus,
  ApplicationError,
  AvailableCommand,
  ConfigOption,
  Model,
  ModelCatalog,
  PlanEntry,
  RuntimeCapabilities,
  RuntimeState,
  Session,
  SessionMode,
  SessionState,
  ToolCallKind,
  Usage,
} from "./contract.ts";
import { advertisedPlanReview } from "./plan-review.ts";
import { isRuntimeUsable } from "./sessions.ts";
import type {
  ApplicationState,
  SessionViewState,
  StreamChunk,
  ToolCallState,
} from "./state.ts";

export const MAX_TERMINAL_DISPLAY_CHARS = 8 * 1024;

export type ConversationSurfaceKind =
  | "no_session"
  | "empty"
  | "active"
  | "closed"
  | "failed"
  | "resync";

export type ComposerKind =
  | "no_session"
  | "unavailable"
  | "ready"
  | "working"
  | "waiting"
  | "cancelling"
  | "cancelled"
  | "completed"
  | "closed"
  | "failed";

export type ConversationCard =
  | {
      type: "user_message";
      id: string;
      text: string;
      truncated: boolean;
    }
  | {
      type: "assistant_message";
      id: string;
      text: string;
      truncated: boolean;
    }
  | {
      type: "thought";
      id: string;
      text: string;
      truncated: boolean;
    }
  | {
      type: "tool";
      id: string;
      title: string;
      kind: ToolCallKind;
      kindLabel: string;
      status: ActivityStatus;
      statusLabel: string;
      detail: string | null;
      detailTruncated: boolean;
      untrusted: boolean;
    }
  | {
      type: "error";
      id: string;
      title: string;
      detail: string;
      recoverable: boolean;
    };

export type ControlChoice = {
  id: string;
  label: string;
  value: string;
};

export type SessionControlPresentation = {
  models: ControlChoice[];
  currentModelId: string | null;
  reasoning: ControlChoice[];
  currentReasoningValue: string | null;
  modes: ControlChoice[];
  currentModeId: string | null;
  configOptions: ConfigOption[];
  commands: AvailableCommand[];
  canChangeModel: boolean;
  canChangeReasoning: boolean;
  canChangeMode: boolean;
  canChangeConfig: boolean;
};

export type ComposerPresentation = {
  kind: ComposerKind;
  label: string;
  detail: string;
  canSend: boolean;
  canCancel: boolean;
  draftEnabled: boolean;
};

export type ActivityPresentation = {
  usage: Usage | null;
  plan: PlanEntry[];
  planHeadline: "empty" | "saved" | "replaced";
  canApprovePlan: boolean;
  canRevisePlan: boolean;
  tools: ToolCallState[];
};

export type ConversationPresentation = {
  kind: ConversationSurfaceKind;
  heading: string;
  detail: string;
  sessionState: SessionState | null;
  cards: ConversationCard[];
  composer: ComposerPresentation;
  controls: SessionControlPresentation;
  activity: ActivityPresentation;
  failure: ApplicationError | null;
  canRecover: boolean;
};

export function projectConversation(input: {
  sessionId: string | null;
  session: Session | null;
  sessionView: SessionViewState | null;
  capabilities: RuntimeCapabilities | null;
  runtimeState: RuntimeState;
  modelCatalog: ModelCatalog | null;
  currentModeId: string | null;
  configOptions: ConfigOption[];
  pendingUserMessages: StreamChunk[];
  sending: boolean;
  cancelling: boolean;
  failure: ApplicationError | null;
  needsResync: boolean;
  runtimeFailure: { diagnostic: string; recoverable: boolean } | null;
}): ConversationPresentation {
  const controls = describeSessionControls({
    session: input.session,
    sessionView: input.sessionView,
    capabilities: input.capabilities,
    modelCatalog: input.modelCatalog,
    currentModeId: input.currentModeId,
    configOptions: input.configOptions,
  });
  const composer = describeComposer({
    sessionId: input.sessionId,
    sessionState: input.sessionView?.state ?? null,
    capabilities: input.capabilities,
    runtimeState: input.runtimeState,
    sending: input.sending,
    cancelling: input.cancelling,
  });
  const cards = projectConversationCards({
    sessionView: input.sessionView,
    pendingUserMessages: input.pendingUserMessages,
    failure: input.failure,
    runtimeFailure: input.runtimeFailure,
  });
  const activity = describeActivity(input.sessionView, composer);
  const canRecover = canRecoverConversation(input);

  if (input.sessionId === null) {
    return {
      kind: "no_session",
      heading: "No session selected",
      detail: "Open or create a session to send a prompt.",
      sessionState: null,
      cards,
      composer,
      controls,
      activity,
      failure: input.failure,
      canRecover: false,
    };
  }
  if (input.needsResync) {
    return {
      kind: "resync",
      heading: "Conversation needs a refresh",
      detail: "Event delivery fell behind. Reopen the session to continue safely.",
      sessionState: input.sessionView?.state ?? null,
      cards,
      composer,
      controls,
      activity,
      failure: input.failure,
      canRecover,
    };
  }
  if (input.sessionView?.state === "closed") {
    return {
      kind: "closed",
      heading: "Session closed",
      detail: "This session is closed. Open another session to keep working.",
      sessionState: "closed",
      cards,
      composer,
      controls,
      activity,
      failure: input.failure,
      canRecover: false,
    };
  }
  if (
    input.sessionView?.state === "failed" ||
    input.runtimeState === "failed" ||
    input.runtimeState === "disconnected"
  ) {
    return {
      kind: "failed",
      heading: input.runtimeState === "disconnected" ? "Disconnected" : "Conversation failed",
      detail:
        input.failure?.diagnostic ??
        input.runtimeFailure?.diagnostic ??
        (input.runtimeState === "disconnected"
          ? "Reconnect to continue this session."
          : "The session or runtime reported a failure."),
      sessionState: input.sessionView?.state ?? null,
      cards,
      composer,
      controls,
      activity,
      failure: input.failure,
      canRecover,
    };
  }
  if (cards.filter((card) => card.type !== "error").length === 0) {
    return {
      kind: "empty",
      heading: "Ready for a prompt",
      detail: "Send a message to start this session.",
      sessionState: input.sessionView?.state ?? "ready",
      cards,
      composer,
      controls,
      activity,
      failure: input.failure,
      canRecover,
    };
  }
  return {
    kind: "active",
    heading: input.sessionView?.title ?? "Conversation",
    detail: describeSessionTurn(input.sessionView?.state ?? "ready"),
    sessionState: input.sessionView?.state ?? "ready",
    cards,
    composer,
    controls,
    activity,
    failure: input.failure,
    canRecover,
  };
}

export function canRecoverConversation(input: {
  runtimeState: RuntimeState;
  failure: ApplicationError | null;
  runtimeFailure: { diagnostic: string; recoverable: boolean } | null;
}): boolean {
  if (input.runtimeState === "disconnected") {
    return true;
  }
  if (input.runtimeState !== "failed") {
    return false;
  }
  if (input.failure?.code === "unsupported_protocol") {
    return false;
  }
  if (input.runtimeFailure !== null && !input.runtimeFailure.recoverable) {
    return false;
  }
  return input.failure?.recoverable ?? true;
}

export function describeComposer(input: {
  sessionId: string | null;
  sessionState: SessionState | null;
  capabilities: RuntimeCapabilities | null;
  runtimeState: RuntimeState;
  sending: boolean;
  cancelling: boolean;
}): ComposerPresentation {
  const canPrompt = input.capabilities?.sessions.prompt ?? false;
  const canCancel = input.capabilities?.sessions.cancel ?? false;
  const runtimeReady = isRuntimeUsable(input.runtimeState);

  if (input.sessionId === null) {
    return composer("no_session", "No session", "Select a session first.", false, false, false);
  }
  if (!runtimeReady || !canPrompt) {
    return composer(
      "unavailable",
      "Prompting unavailable",
      runtimeReady
        ? "This runtime does not advertise prompting."
        : "Reconnect before sending a prompt.",
      false,
      false,
      false,
    );
  }
  if (input.sessionState === "closed") {
    return composer("closed", "Session closed", "This session cannot take a prompt.", false, false, false);
  }
  if (input.sessionState === "failed") {
    return composer("failed", "Session failed", "The session cannot take another prompt.", false, false, false);
  }
  if (input.cancelling || input.sessionState === "cancelling") {
    return composer("cancelling", "Cancelling", "The current turn is stopping.", false, false, false);
  }
  if (input.sessionState === "cancelled") {
    return composer(
      "cancelled",
      "Cancelled",
      "The last turn was cancelled.",
      true,
      false,
      true,
    );
  }
  if (input.sessionState === "completed") {
    return composer("completed", "Completed", "The last turn completed.", true, false, true);
  }
  if (input.sending || input.sessionState === "working") {
    return composer(
      "working",
      "Working",
      "The agent is working on this turn.",
      false,
      canCancel,
      false,
    );
  }
  if (input.sessionState === "waiting_for_input") {
    return composer(
      "waiting",
      "Waiting for input",
      "The agent is waiting for the next message.",
      true,
      canCancel,
      true,
    );
  }
  return composer("ready", "Ready", "Send a prompt to continue.", true, false, true);
}

export function describeSessionControls(input: {
  session: Session | null;
  sessionView: SessionViewState | null;
  capabilities: RuntimeCapabilities | null;
  modelCatalog: ModelCatalog | null;
  currentModeId: string | null;
  configOptions: ConfigOption[];
}): SessionControlPresentation {
  const catalog = input.modelCatalog ?? input.session?.models ?? input.capabilities?.models ?? null;
  const models = (catalog?.availableModels ?? []).map((model) => ({
    id: model.modelId,
    label: model.name === "" ? model.modelId : model.name,
    value: model.modelId,
  }));
  const currentModelId = catalog?.currentModelId ?? null;
  const selectedModel = selectedModelFromCatalog(catalog, currentModelId);
  const reasoning = (selectedModel?.reasoningEfforts ?? []).map((effort) => ({
    id: effort.id,
    label: effort.label === "" ? effort.value : effort.label,
    value: effort.value,
  }));
  const advertisedModes =
    input.session?.controls.modes?.availableModes ?? [];
  const modes = advertisedModes.map((mode) => controlFromMode(mode));
  const currentModeId =
    input.currentModeId ??
    input.sessionView?.currentModeId ??
    input.session?.controls.modes?.currentModeId ??
    null;
  const configOptions =
    input.configOptions.length > 0
      ? input.configOptions
      : input.sessionView?.configOptions.length
        ? input.sessionView.configOptions
        : (input.session?.controls.configOptions ?? []);
  const commands = input.sessionView?.availableCommands ?? [];

  return {
    models,
    currentModelId: models.some((model) => model.id === currentModelId) ? currentModelId : null,
    reasoning,
    currentReasoningValue: selectedReasoningValue(selectedModel),
    modes,
    currentModeId: modes.some((mode) => mode.id === currentModeId) ? currentModeId : null,
    configOptions,
    commands,
    canChangeModel: models.length > 0,
    canChangeReasoning: reasoning.length > 0,
    canChangeMode: modes.length > 0,
    canChangeConfig: configOptions.length > 0,
  };
}

export function projectConversationCards(input: {
  sessionView: SessionViewState | null;
  pendingUserMessages: StreamChunk[];
  failure: ApplicationError | null;
  runtimeFailure: { diagnostic: string; recoverable: boolean } | null;
}): ConversationCard[] {
  const cards: ConversationCard[] = [];
  const session = input.sessionView;
  if (session !== null) {
    for (const item of session.timeline) {
      if (item.kind === "user_message" || item.kind === "assistant_message") {
        const chunk = session.messages[item.id];
        if (chunk === undefined) {
          continue;
        }
        cards.push({
          type: item.kind,
          id: chunk.id,
          text: chunk.text,
          truncated: chunk.truncated,
        });
        continue;
      }
      if (item.kind === "thought") {
        const chunk = session.thoughts[item.id];
        if (chunk === undefined) {
          continue;
        }
        cards.push({
          type: "thought",
          id: chunk.id,
          text: chunk.text,
          truncated: chunk.truncated,
        });
        continue;
      }
      const tool = session.toolCalls[item.id];
      if (tool !== undefined) {
        cards.push(toolCard(tool));
      }
    }
  }

  const seenUserText = new Set(
    cards.filter((card) => card.type === "user_message").map((card) => card.text),
  );
  for (const pending of input.pendingUserMessages) {
    if (seenUserText.has(pending.text)) {
      continue;
    }
    cards.push({
      type: "user_message",
      id: pending.id,
      text: pending.text,
      truncated: pending.truncated,
    });
  }

  if (input.failure !== null) {
    cards.push({
      type: "error",
      id: `error:${input.failure.code}`,
      title: "Request failed",
      detail: input.failure.diagnostic,
      recoverable: input.failure.recoverable,
    });
  }
  if (session?.state === "failed") {
    cards.push({
      type: "error",
      id: "error:session",
      title: "Session failed",
      detail: "This session reported a failure.",
      recoverable: true,
    });
  }
  if (input.runtimeFailure !== null) {
    cards.push({
      type: "error",
      id: "error:runtime",
      title: "Runtime failed",
      detail: input.runtimeFailure.diagnostic,
      recoverable: input.runtimeFailure.recoverable,
    });
  }
  return cards;
}

export function describeActivity(
  sessionView: SessionViewState | null,
  composer?: ComposerPresentation,
): ActivityPresentation {
  const review = advertisedPlanReview(sessionView?.availableCommands ?? []);
  const canReview = composer?.canSend ?? false;
  const plan = sessionView?.plan ?? [];
  const revision = sessionView?.planRevision ?? 0;
  return {
    usage: sessionView?.usage ?? null,
    plan,
    planHeadline: plan.length === 0 ? "empty" : revision > 1 ? "replaced" : "saved",
    canApprovePlan: canReview && review.approve !== null && plan.length > 0,
    canRevisePlan: canReview && review.revise !== null && plan.length > 0,
    tools: sessionView === null ? [] : Object.values(sessionView.toolCalls),
  };
}

export function describeToolKind(kind: ToolCallKind): string {
  switch (kind) {
    case "tool":
      return "Tool";
    case "terminal_command":
      return "Terminal";
    case "background_task":
      return "Background";
    case "subagent":
      return "Subagent";
    case "other":
      return "Activity";
  }
}

export function describeActivityStatus(status: ActivityStatus): string {
  switch (status) {
    case "pending":
      return "Pending";
    case "running":
      return "Running";
    case "waiting_for_input":
      return "Needs input";
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Cancelled";
  }
}

export function describeSessionTurn(state: SessionState): string {
  switch (state) {
    case "creating":
      return "Creating the session.";
    case "ready":
      return "Ready for the next prompt.";
    case "working":
      return "The agent is working.";
    case "waiting_for_input":
      return "The agent is waiting for input.";
    case "cancelling":
      return "The current turn is cancelling.";
    case "cancelled":
      return "The last turn was cancelled.";
    case "completed":
      return "The last turn completed.";
    case "closed":
      return "This session is closed.";
    case "failed":
      return "This session failed.";
  }
}

export function boundUntrustedText(
  value: string | null,
  maximum = MAX_TERMINAL_DISPLAY_CHARS,
): { text: string | null; truncated: boolean } {
  if (value === null) {
    return { text: null, truncated: false };
  }
  if (value.length <= maximum) {
    return { text: value, truncated: false };
  }
  return { text: value.slice(0, maximum), truncated: true };
}

function toolCard(tool: ToolCallState): Extract<ConversationCard, { type: "tool" }> {
  const untrusted = tool.kind === "terminal_command";
  const bounded = boundUntrustedText(tool.detail);
  return {
    type: "tool",
    id: tool.id,
    title: tool.title,
    kind: tool.kind,
    kindLabel: describeToolKind(tool.kind),
    status: tool.status,
    statusLabel: describeActivityStatus(tool.status),
    detail: bounded.text,
    detailTruncated: bounded.truncated,
    untrusted,
  };
}

function selectedModelFromCatalog(
  catalog: ModelCatalog | null,
  currentModelId: string | null,
): Model | null {
  if (catalog === null) {
    return null;
  }
  return (
    catalog.availableModels.find((model) => model.modelId === currentModelId) ??
    catalog.availableModels[0] ??
    null
  );
}

function selectedReasoningValue(model: Model | null): string | null {
  if (model === null) {
    return null;
  }
  if (model.reasoningEffort !== null && model.reasoningEffort !== "") {
    return model.reasoningEffort;
  }
  return model.reasoningEfforts.find((effort) => effort.isDefault)?.value ?? null;
}

function controlFromMode(mode: SessionMode): ControlChoice {
  return {
    id: mode.id,
    label: mode.name === "" ? mode.id : mode.name,
    value: mode.id,
  };
}

function composer(
  kind: ComposerKind,
  label: string,
  detail: string,
  canSend: boolean,
  canCancel: boolean,
  draftEnabled: boolean,
): ComposerPresentation {
  return { kind, label, detail, canSend, canCancel, draftEnabled };
}

export function conversationFromApplication(
  application: ApplicationState,
  sessionId: string | null,
): SessionViewState | null {
  if (sessionId === null) {
    return null;
  }
  return application.sessions[sessionId] ?? null;
}
