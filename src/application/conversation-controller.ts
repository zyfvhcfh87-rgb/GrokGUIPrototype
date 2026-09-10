import type {
  Acknowledgement,
  ApplicationError,
  ApplicationEventEnvelope,
  ConfigOption,
  ConfigValue,
  ModelCatalog,
  OpenExternalUrlRequest,
  PromptRequest,
  PromptResult,
  RuntimeCapabilities,
  RuntimeSnapshot,
  RuntimeState,
  Session,
  SessionRequest,
  SetSessionConfigRequest,
  SetSessionModeRequest,
  SetSessionModelRequest,
} from "./contract.ts";
import {
  conversationFromApplication,
  projectConversation,
  type ConversationPresentation,
} from "./conversation.ts";
import {
  formatPlanCommand,
  planReviewCommand,
  type PlanReviewAction,
} from "./plan-review.ts";
import {
  initialApplicationState,
  reduceApplicationEvent,
  type ApplicationState,
  type StreamChunk,
} from "./state.ts";

type Unlisten = () => void;

export type ConversationControllerBridge = {
  onEvent(listener: (event: ApplicationEventEnvelope) => void): Promise<Unlisten>;
  runtimeSnapshot?(): Promise<RuntimeSnapshot>;
  sendPrompt(request: PromptRequest): Promise<PromptResult>;
  cancelPrompt(request: SessionRequest): Promise<Acknowledgement>;
  setSessionMode(request: SetSessionModeRequest): Promise<Acknowledgement>;
  setSessionModel(request: SetSessionModelRequest): Promise<Acknowledgement>;
  setSessionConfig(request: SetSessionConfigRequest): Promise<Acknowledgement>;
  openExternalUrl?(request: OpenExternalUrlRequest): Promise<Acknowledgement>;
};

export type ConversationControllerState = {
  sessionId: string | null;
  session: Session | null;
  capabilities: RuntimeCapabilities | null;
  runtimeState: RuntimeState;
  application: ApplicationState;
  modelCatalog: ModelCatalog | null;
  currentModeId: string | null;
  configOptions: ConfigOption[];
  draft: string;
  sending: boolean;
  cancelling: boolean;
  controlBusy: boolean;
  failure: ApplicationError | null;
  pendingUserMessages: StreamChunk[];
  interactionEpoch: number;
};

const INITIAL_STATE: ConversationControllerState = {
  sessionId: null,
  session: null,
  capabilities: null,
  runtimeState: "disconnected",
  application: initialApplicationState(),
  modelCatalog: null,
  currentModeId: null,
  configOptions: [],
  draft: "",
  sending: false,
  cancelling: false,
  controlBusy: false,
  failure: null,
  pendingUserMessages: [],
  interactionEpoch: 0,
};

export function createConversationController(bridge: ConversationControllerBridge) {
  let state = INITIAL_STATE;
  let unlisten: Unlisten | null = null;
  let lifecycle = 0;
  let pendingSerial = 0;
  const listeners = new Set<(state: ConversationControllerState) => void>();

  const publish = (patch: Partial<ConversationControllerState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) {
      listener(state);
    }
  };

  const adoptSnapshot = (snapshot: RuntimeSnapshot) => {
    const current = state.application;
    if (snapshot.generation > current.generation) {
      return {
        ...initialApplicationState(snapshot.generation),
        lastSequence: snapshot.lastSequence,
        runtime: {
          ...initialApplicationState().runtime,
          state: snapshot.state,
        },
      };
    }
    if (
      snapshot.generation === current.generation &&
      current.lastSequence === 0 &&
      current.pendingEventCount === 0
    ) {
      return {
        ...current,
        lastSequence: snapshot.lastSequence,
        runtime: { ...current.runtime, state: snapshot.state },
      };
    }
    return current;
  };

  const handleEvent = (envelope: ApplicationEventEnvelope) => {
    const application = reduceApplicationEvent(state.application, envelope);
    const event = envelope.event;
    let currentModeId = state.currentModeId;
    let configOptions = state.configOptions;
    let pendingUserMessages = state.pendingUserMessages;
    if (event.type === "session_mode_changed" && event.sessionId === state.sessionId) {
      currentModeId = event.currentModeId;
    }
    if (event.type === "session_config_options_changed" && event.sessionId === state.sessionId) {
      configOptions = event.configOptions;
    }
    if (event.type === "user_message_chunk_received" && event.sessionId === state.sessionId) {
      const view = conversationFromApplication(application, state.sessionId);
      const texts = new Set(Object.values(view?.messages ?? {}).map((chunk) => chunk.text));
      pendingUserMessages = pendingUserMessages.filter((chunk) => !texts.has(chunk.text));
    }
    let { cancelling, sending, runtimeState } = state;
    if (envelope.generation > state.application.generation) {
      pendingUserMessages = [];
      runtimeState = application.runtime.state;
      cancelling = false;
      sending = false;
    }
    if (event.type === "runtime_state_changed" || event.type === "runtime_failed") {
      runtimeState = application.runtime.state;
    }
    if (event.type === "session_state_changed" && event.sessionId === state.sessionId) {
      if (
        event.state === "cancelled" ||
        event.state === "completed" ||
        event.state === "failed" ||
        event.state === "closed"
      ) {
        cancelling = false;
        sending = false;
      }
    }
    if (
      event.type === "runtime_failed" ||
      (event.type === "runtime_state_changed" &&
        (event.state === "failed" || event.state === "disconnected"))
    ) {
      cancelling = false;
      sending = false;
    }
    publish({
      application,
      runtimeState,
      currentModeId,
      configOptions,
      pendingUserMessages,
      cancelling,
      sending,
    });
  };

  const requireSessionId = (): string => {
    if (state.sessionId === null) {
      throw applicationError(
        {
          code: "invalid_request",
          diagnostic: "Select a session before sending a prompt.",
          recoverable: true,
        },
        "Select a session before sending a prompt.",
        "invalid_request",
      );
    }
    return state.sessionId;
  };

  const submitPrompt = async (text: string) => {
    const sessionId = requireSessionId();
    if (text === "") {
      return null;
    }
    if (!(state.capabilities?.sessions.prompt ?? false)) {
      const failure = applicationError(
        {
          code: "capability_unavailable",
          diagnostic: "This runtime does not advertise prompting.",
          recoverable: false,
        },
        "This runtime does not advertise prompting.",
        "capability_unavailable",
      );
      publish({ failure });
      throw failure;
    }
    const pending: StreamChunk = {
      id: `local-user:${(pendingSerial += 1)}`,
      text,
      truncated: false,
    };
    publish({
      sending: true,
      draft: "",
      failure: null,
      pendingUserMessages: [...state.pendingUserMessages, pending],
    });
    try {
      const result = await bridge.sendPrompt({ sessionId, text });
      if (state.sessionId === sessionId) {
        publish({ sending: false, cancelling: false });
      }
      return result;
    } catch (error) {
      const failure = applicationError(error, "The prompt could not be sent.");
      if (state.sessionId === sessionId) {
        publish({
          sending: false,
          cancelling: false,
          draft: state.draft === "" ? text : state.draft,
          failure,
          pendingUserMessages: state.pendingUserMessages.filter((chunk) => chunk.id !== pending.id),
        });
      }
      throw failure;
    }
  };

  return {
    getState: () => state,
    subscribe: (listener: (state: ConversationControllerState) => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    presentation: (): ConversationPresentation =>
      projectConversation({
        sessionId: state.sessionId,
        session: state.session,
        sessionView: conversationFromApplication(state.application, state.sessionId),
        capabilities: state.capabilities,
        runtimeState: state.runtimeState,
        modelCatalog: state.modelCatalog,
        currentModeId: state.currentModeId,
        configOptions: state.configOptions,
        pendingUserMessages: state.pendingUserMessages,
        sending: state.sending,
        cancelling: state.cancelling,
        failure: state.failure,
        needsResync: state.application.needsResync,
        runtimeFailure: state.application.runtime.failure,
      }),
    initialize: async () => {
      if (unlisten !== null) {
        return;
      }
      const currentLifecycle = ++lifecycle;
      const nextUnlisten = await bridge.onEvent(handleEvent);
      if (currentLifecycle !== lifecycle) {
        nextUnlisten();
        return;
      }
      unlisten = nextUnlisten;
      if (bridge.runtimeSnapshot === undefined) {
        return;
      }
      try {
        const snapshot = await bridge.runtimeSnapshot();
        if (currentLifecycle !== lifecycle) {
          return;
        }
        publish({
          application: adoptSnapshot(snapshot),
          runtimeState: snapshot.state,
          capabilities: snapshot.capabilities,
          modelCatalog: state.session?.models ?? snapshot.capabilities?.models ?? state.modelCatalog,
        });
      } catch {
        return;
      }
    },
    setRuntime: (runtimeState: RuntimeState, capabilities: RuntimeCapabilities | null) => {
      publish({
        interactionEpoch: state.interactionEpoch + (runtimeState !== state.runtimeState && ["connecting", "authenticating", "failed", "disconnected"].includes(runtimeState) ? 1 : 0),
        runtimeState,
        capabilities,
        modelCatalog: state.session?.models ?? capabilities?.models ?? state.modelCatalog,
      });
    },
    setSession: (session: Session | null) => {
      publish({
        interactionEpoch: state.interactionEpoch + (session !== state.session ? 1 : 0),
        sessionId: session?.sessionId ?? null,
        session,
        modelCatalog: session?.models ?? state.modelCatalog,
        currentModeId: session?.controls.modes?.currentModeId ?? null,
        configOptions: session?.controls.configOptions ?? [],
        draft: "",
        sending: false,
        cancelling: false,
        controlBusy: false,
        failure: null,
        pendingUserMessages: [],
      });
    },
    setDraft: (draft: string) => {
      publish({ draft });
    },
    sendPrompt: async () => submitPrompt(state.draft.trim()),
    reviewPlan: async (action: PlanReviewAction) => {
      const sessionId = requireSessionId();
      const commands =
        conversationFromApplication(state.application, sessionId)?.availableCommands ?? [];
      const command = planReviewCommand(commands, action);
      if (command === null) {
        const failure = applicationError(
          {
            code: "capability_unavailable",
            diagnostic: "This runtime does not advertise plan review.",
            recoverable: false,
          },
          "This runtime does not advertise plan review.",
          "capability_unavailable",
        );
        publish({ failure });
        throw failure;
      }
      const draft = formatPlanCommand(command);
      if (action === "revise" && command.acceptsInput) {
        publish({ draft, failure: null });
        return null;
      }
      return submitPrompt(draft.trim());
    },
    cancelPrompt: async () => {
      const sessionId = requireSessionId();
      if (!(state.capabilities?.sessions.cancel ?? false)) {
        const failure = applicationError(
          {
            code: "capability_unavailable",
            diagnostic: "This runtime does not advertise cancellation.",
            recoverable: false,
          },
          "This runtime does not advertise cancellation.",
          "capability_unavailable",
        );
        publish({ failure });
        throw failure;
      }
      const sessionState = conversationFromApplication(state.application, sessionId)?.state ?? null;
      if (state.cancelling || sessionState === "cancelling") {
        return { acknowledged: true };
      }
      if (
        !state.sending &&
        sessionState !== "working" &&
        sessionState !== "waiting_for_input"
      ) {
        return { acknowledged: true };
      }
      publish({ cancelling: true, failure: null, interactionEpoch: state.interactionEpoch + 1 });
      try {
        await bridge.cancelPrompt({ sessionId });
        return { acknowledged: true };
      } catch (error) {
        const failure = applicationError(error, "The current turn could not be cancelled.");
        if (state.sessionId === sessionId) {
          publish({ cancelling: false, failure });
        }
        throw failure;
      }
    },
    setMode: async (modeId: string) => {
      const sessionId = requireSessionId();
      publish({ controlBusy: true, failure: null });
      try {
        await bridge.setSessionMode({ sessionId, modeId });
        if (state.sessionId === sessionId) {
          publish({ controlBusy: false, currentModeId: modeId });
        }
      } catch (error) {
        const failure = applicationError(error, "The session mode could not be changed.");
        if (state.sessionId === sessionId) {
          publish({ controlBusy: false, failure });
        }
        throw failure;
      }
    },
    setModel: async (modelId: string, reasoningEffort: string | null) => {
      const sessionId = requireSessionId();
      publish({ controlBusy: true, failure: null });
      try {
        await bridge.setSessionModel({ sessionId, modelId, reasoningEffort });
        if (state.sessionId === sessionId) {
          publish({
            controlBusy: false,
            modelCatalog: applyModelSelection(state.modelCatalog, modelId, reasoningEffort),
          });
        }
      } catch (error) {
        const failure = applicationError(error, "The model could not be changed.");
        if (state.sessionId === sessionId) {
          publish({ controlBusy: false, failure });
        }
        throw failure;
      }
    },
    setConfig: async (configId: string, value: ConfigValue) => {
      const sessionId = requireSessionId();
      publish({ controlBusy: true, failure: null });
      try {
        await bridge.setSessionConfig({ sessionId, configId, value });
        if (state.sessionId === sessionId) {
          publish({
            controlBusy: false,
            configOptions: applyConfigValue(state.configOptions, configId, value),
          });
        }
      } catch (error) {
        const failure = applicationError(error, "That setting could not be changed.");
        if (state.sessionId === sessionId) {
          publish({ controlBusy: false, failure });
        }
        throw failure;
      }
    },
    insertCommand: (name: string, acceptsInput: boolean) => {
      const prefix = name.startsWith("/") ? name : `/${name}`;
      publish({ draft: acceptsInput ? `${prefix} ` : prefix });
    },
    openExternalUrl: async (url: string) => {
      if (bridge.openExternalUrl === undefined) {
        const failure = applicationError(
          {
            code: "capability_unavailable",
            diagnostic: "External links cannot be opened in this build.",
            recoverable: false,
          },
          "External links cannot be opened in this build.",
          "capability_unavailable",
        );
        publish({ failure });
        throw failure;
      }
      try {
        await bridge.openExternalUrl({ url });
      } catch (error) {
        const failure = applicationError(error, "That link could not be opened.");
        publish({ failure });
        throw failure;
      }
    },
    dispose: () => {
      lifecycle += 1;
      unlisten?.();
      unlisten = null;
      listeners.clear();
    },
  } as const;
}

function applyModelSelection(
  catalog: ModelCatalog | null,
  modelId: string,
  reasoningEffort: string | null,
): ModelCatalog | null {
  if (catalog === null) {
    return null;
  }
  return {
    ...catalog,
    currentModelId: modelId,
    availableModels: catalog.availableModels.map((model) =>
      model.modelId === modelId ? { ...model, reasoningEffort } : model,
    ),
  };
}

function applyConfigValue(
  options: ConfigOption[],
  configId: string,
  value: ConfigValue,
): ConfigOption[] {
  return options.map((option) => {
    if (option.id !== configId) {
      return option;
    }
    if (option.kind.type === "select" && value.type === "select") {
      return { ...option, kind: { ...option.kind, currentValue: value.value } };
    }
    if (option.kind.type === "boolean" && value.type === "boolean") {
      return { ...option, kind: { ...option.kind, currentValue: value.value } };
    }
    return option;
  });
}

function applicationError(
  value: unknown,
  fallbackDiagnostic: string,
  fallbackCode: ApplicationError["code"] = "protocol_request_failed",
): ApplicationError {
  if (isApplicationError(value)) {
    return value;
  }
  return {
    code: fallbackCode,
    diagnostic: fallbackDiagnostic,
    recoverable: true,
  };
}

function isApplicationError(value: unknown): value is ApplicationError {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Partial<ApplicationError>;
  return (
    typeof candidate.code === "string" &&
    typeof candidate.diagnostic === "string" &&
    typeof candidate.recoverable === "boolean"
  );
}
