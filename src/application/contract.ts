export const APPLICATION_EVENT_NAME = "grok-application-event" as const;

export const APPLICATION_COMMANDS = {
  setupStatus: "setup_status",
  validateWorkspace: "workspace_validate",
  runtimeSnapshot: "runtime_snapshot",
  startRuntime: "runtime_start",
  stopRuntime: "runtime_stop",
  restartRuntime: "runtime_restart",
  newSession: "session_new",
  listSessions: "session_list",
  loadSession: "session_load",
  resumeSession: "session_resume",
  closeSession: "session_close",
  sendPrompt: "prompt_send",
  cancelPrompt: "prompt_cancel",
  setSessionMode: "session_set_mode",
  setSessionModel: "session_set_model",
  setSessionConfig: "session_set_config",
  respondToPermission: "permission_respond",
  respondToElicitation: "elicitation_respond",
} as const;

export type RuntimeState =
  | "connecting"
  | "authenticating"
  | "ready"
  | "working"
  | "waiting_for_input"
  | "failed"
  | "disconnected";

export type SessionState =
  | "creating"
  | "ready"
  | "working"
  | "waiting_for_input"
  | "cancelling"
  | "completed"
  | "closed"
  | "failed";

export type ActivityStatus =
  | "pending"
  | "running"
  | "waiting_for_input"
  | "completed"
  | "failed"
  | "cancelled";

export type ToolCallKind =
  | "tool"
  | "terminal_command"
  | "background_task"
  | "subagent"
  | "other";

export type PermissionScope =
  | { type: "tool"; toolName: string }
  | {
      type: "command";
      command: string;
      workingDirectory: string | null;
    }
  | { type: "filesystem"; operation: string; path: string }
  | { type: "network"; destination: string }
  | { type: "other" };

export type ElicitationControl =
  | {
      type: "text";
      fieldId: string;
      label: string | null;
      placeholder: string | null;
      sensitive: boolean;
    }
  | { type: "confirmation"; fieldId: string; label: string | null }
  | {
      type: "choice";
      fieldId: string;
      label: string | null;
      options: string[];
      multiple: boolean;
      truncated: boolean;
    }
  | { type: "other" };

export type PlanEntry = {
  id: string;
  title: string;
  description: string | null;
  status: "pending" | "in_progress" | "completed";
};

export type Usage = {
  inputTokens: number | null;
  outputTokens: number | null;
  cachedInputTokens: number | null;
  totalTokens: number | null;
  contextWindowTokens: number | null;
};

export type AvailableCommand = {
  name: string;
  description: string;
  acceptsInput: boolean;
};

export type OptionalUpdate =
  | { state: "not_reported" }
  | { state: "cleared" }
  | { state: "value"; value: string };

export type ConfigChoice = {
  value: string;
  name: string;
  description: string | null;
  group: string | null;
};

export type ConfigOptionKind =
  | {
      type: "select";
      currentValue: string;
      options: ConfigChoice[];
      truncated: boolean;
    }
  | { type: "boolean"; currentValue: boolean };

export type ConfigOption = {
  id: string;
  name: string;
  description: string | null;
  kind: ConfigOptionKind;
};

export type RuntimeExtensionArea =
  | "models"
  | "settings"
  | "sessions"
  | "queue"
  | "prompt_completion"
  | "session"
  | "announcements"
  | "mcp"
  | "malformed";

export type ApplicationEvent =
  | { type: "runtime_state_changed"; state: RuntimeState }
  | {
      type: "session_state_changed";
      sessionId: string;
      state: SessionState;
    }
  | {
      type: "user_message_chunk_received";
      sessionId: string;
      messageId: string | null;
      text: string;
      truncated: boolean;
    }
  | {
      type: "message_chunk_received";
      sessionId: string;
      messageId: string | null;
      text: string;
      truncated: boolean;
    }
  | {
      type: "thought_chunk_received";
      sessionId: string;
      thoughtId: string | null;
      text: string;
      truncated: boolean;
    }
  | {
      type: "tool_call_changed";
      sessionId: string;
      callId: string;
      title: string;
      kind: ToolCallKind;
      status: ActivityStatus;
      detail: string | null;
    }
  | {
      type: "permission_requested";
      sessionId: string;
      interactionId: string;
      title: string;
      consequence: string | null;
      scope: PermissionScope;
      availableDecisions: PermissionDecision[];
    }
  | {
      type: "elicitation_requested";
      sessionId: string | null;
      interactionId: string;
      prompt: string;
      control: ElicitationControl;
    }
  | {
      type: "plan_changed";
      sessionId: string;
      entries: PlanEntry[];
      truncated: boolean;
    }
  | { type: "usage_changed"; sessionId: string; usage: Usage }
  | { type: "session_metadata_changed"; sessionId: string }
  | {
      type: "available_commands_changed";
      sessionId: string;
      commands: AvailableCommand[];
      truncated: boolean;
    }
  | {
      type: "session_mode_changed";
      sessionId: string;
      currentModeId: string;
    }
  | {
      type: "session_config_options_changed";
      sessionId: string;
      configOptions: ConfigOption[];
      truncated: boolean;
    }
  | {
      type: "session_info_changed";
      sessionId: string;
      title: OptionalUpdate;
      updatedAt: OptionalUpdate;
    }
  | { type: "runtime_failed"; diagnostic: string; recoverable: boolean }
  | {
      type: "runtime_extension_invalidated";
      area: RuntimeExtensionArea;
      sessionId: string | null;
    }
  | {
      type: "extension_observed";
      classification: "unknown" | "invalid";
    }
  | { type: "session_activated"; sessionId: string }
  | {
      type: "interaction_resolved";
      interactionId: string;
      kind: "permission" | "elicitation";
    }
  | { type: "interactions_cleared"; sessionId: string | null };

type ApplicationEventFieldMap = {
  [Event in ApplicationEvent as Event["type"]]: readonly Exclude<keyof Event, "type">[];
};

export const APPLICATION_EVENT_FIELDS = {
  runtime_state_changed: ["state"],
  session_state_changed: ["sessionId", "state"],
  user_message_chunk_received: ["messageId", "sessionId", "text", "truncated"],
  message_chunk_received: ["messageId", "sessionId", "text", "truncated"],
  thought_chunk_received: ["sessionId", "text", "thoughtId", "truncated"],
  tool_call_changed: ["callId", "detail", "kind", "sessionId", "status", "title"],
  permission_requested: [
    "availableDecisions",
    "consequence",
    "interactionId",
    "scope",
    "sessionId",
    "title",
  ],
  elicitation_requested: ["control", "interactionId", "prompt", "sessionId"],
  plan_changed: ["entries", "sessionId", "truncated"],
  usage_changed: ["sessionId", "usage"],
  session_metadata_changed: ["sessionId"],
  available_commands_changed: ["commands", "sessionId", "truncated"],
  session_mode_changed: ["currentModeId", "sessionId"],
  session_config_options_changed: ["configOptions", "sessionId", "truncated"],
  session_info_changed: ["sessionId", "title", "updatedAt"],
  runtime_failed: ["diagnostic", "recoverable"],
  runtime_extension_invalidated: ["area", "sessionId"],
  extension_observed: ["classification"],
  session_activated: ["sessionId"],
  interaction_resolved: ["interactionId", "kind"],
  interactions_cleared: ["sessionId"],
} as const satisfies ApplicationEventFieldMap;

export type ApplicationEventEnvelope = {
  generation: number;
  sequence: number;
  event: ApplicationEvent;
};

export type ApplicationErrorCode =
  | "executable_unavailable"
  | "process_start_failed"
  | "connection_failed"
  | "request_timed_out"
  | "initialize_failed"
  | "unsupported_protocol"
  | "unsupported_authentication"
  | "authentication_failed"
  | "runtime_stopped"
  | "invalid_workspace"
  | "invalid_request"
  | "capability_unavailable"
  | "protocol_request_failed"
  | "malformed_response"
  | "unknown_interaction"
  | "decision_unavailable"
  | "unexpected_response"
  | "boundary_violation";

export type ApplicationError = {
  code: ApplicationErrorCode;
  diagnostic: string;
  recoverable: boolean;
};

export type WorkspaceRequest = { path: string };
export type Workspace = { path: string };
export type NewSessionRequest = { workspace: string };
export type ListSessionsRequest = {
  workspace: string | null;
  cursor: string | null;
};
export type SessionWorkspaceRequest = {
  sessionId: string;
  workspace: string;
};
export type SessionRequest = { sessionId: string };
export type PromptRequest = { sessionId: string; text: string };
export type SetSessionModeRequest = { sessionId: string; modeId: string };
export type SetSessionModelRequest = {
  sessionId: string;
  modelId: string;
  reasoningEffort: string | null;
};
export type ConfigValue =
  | { type: "select"; value: string }
  | { type: "boolean"; value: boolean };
export type SetSessionConfigRequest = {
  sessionId: string;
  configId: string;
  value: ConfigValue;
};
export type PermissionDecision =
  | "allow_once"
  | "allow_always"
  | "deny_once"
  | "deny_always"
  | "cancel";
export type PermissionResponseRequest = {
  interactionId: string;
  decision: PermissionDecision;
};
export type ElicitationValue =
  | { type: "string"; value: string }
  | { type: "integer"; value: number }
  | { type: "number"; value: number }
  | { type: "boolean"; value: boolean }
  | { type: "string_array"; value: string[] };
export type ElicitationDecision =
  | {
      type: "accept";
      value: { content: Record<string, ElicitationValue> };
    }
  | { type: "decline" }
  | { type: "cancel" };
export type ElicitationResponseRequest = {
  interactionId: string;
  decision: ElicitationDecision;
};

export type SetupStatus = {
  runtimeAvailable: boolean;
  failure: ApplicationError | null;
};
export type Acknowledgement = { acknowledged: boolean };
export type PromptResult = {
  stopReason:
    | "end_turn"
    | "max_tokens"
    | "max_turn_requests"
    | "refusal"
    | "cancelled"
    | "other";
};

export type RuntimeSnapshot = {
  generation: number;
  lastSequence: number;
  state: RuntimeState;
  capabilities: RuntimeCapabilities | null;
};
export type RuntimeCapabilities = {
  protocolVersion: number;
  agent: RuntimeAgent;
  authenticationMethods: AuthenticationMethod[];
  sessions: RuntimeSessionCapabilities;
  models: ModelCatalog | null;
  truncated: boolean;
};
export type RuntimeAgent = {
  product: "grok_build" | "other";
  version: string | null;
};
export type AuthenticationMethod =
  | "cached_token"
  | "grok_com"
  | "xai_api_key"
  | "grok"
  | "other";
export type RuntimeSessionCapabilities = {
  create: boolean;
  prompt: boolean;
  cancel: boolean;
  list: boolean;
  load: boolean;
  resume: boolean;
  close: boolean;
};
export type ModelCatalog = {
  currentModelId: string;
  availableModels: Model[];
  truncated: boolean;
};
export type Model = {
  modelId: string;
  name: string;
  description: string | null;
  agentType: string | null;
  reasoningEffort: string | null;
  reasoningEfforts: ReasoningEffort[];
  supportsReasoningEffort: boolean | null;
  totalContextTokens: number | null;
  truncated: boolean;
};
export type ReasoningEffort = {
  id: string;
  label: string;
  description: string | null;
  value: string;
  isDefault: boolean;
};
export type Session = {
  sessionId: string;
  models: ModelCatalog | null;
  legacyConfigOptions: LegacyConfigOption[];
  controls: SessionControls;
  truncated: boolean;
};
export type LegacyConfigOption = {
  id: string;
  label: string;
  description: string | null;
  category: string | null;
  selected: boolean | null;
};
export type SessionControls = {
  modes: SessionModes | null;
  configOptions: ConfigOption[];
  truncated: boolean;
};
export type SessionModes = {
  currentModeId: string;
  availableModes: SessionMode[];
  truncated: boolean;
};
export type SessionMode = {
  id: string;
  name: string;
  description: string | null;
};
export type SessionPage = {
  sessions: SessionSummary[];
  nextCursor: string | null;
  truncated: boolean;
};
export type SessionSummary = {
  sessionId: string;
  workspace: string;
  title: string | null;
  updatedAt: string | null;
};

type StringKeyOf<Value> = Extract<keyof Value, string>;

const fieldsOf = <Value>() =>
  <const Fields extends readonly StringKeyOf<Value>[]>(
    fields: Fields &
      (Exclude<StringKeyOf<Value>, Fields[number]> extends never
        ? unknown
        : readonly ["missing fields", Exclude<StringKeyOf<Value>, Fields[number]>]),
  ) => fields;

const taggedFieldsOf = <Value, Tag extends StringKeyOf<Value>>() =>
  <const Fields extends readonly Exclude<StringKeyOf<Value>, Tag>[]>(
    fields: Fields &
      (Exclude<Exclude<StringKeyOf<Value>, Tag>, Fields[number]> extends never
        ? unknown
        : readonly [
            "missing fields",
            Exclude<Exclude<StringKeyOf<Value>, Tag>, Fields[number]>,
          ]),
  ) => fields;

type TaggedVariantFieldMap<
  Tag extends PropertyKey,
  Union extends Record<Tag, PropertyKey>,
> = {
  [Variant in Union as Variant[Tag]]: readonly Exclude<
    StringKeyOf<Variant>,
    Extract<Tag, string>
  >[];
};

const valuesOf = <Value extends string>() =>
  <const Values extends readonly Value[]>(
    values: Values &
      (Exclude<Value, Values[number]> extends never
        ? unknown
        : readonly ["missing values", Exclude<Value, Values[number]>]),
  ) => values;

export const APPLICATION_DTO_FIELDS = {
  applicationEventEnvelope: fieldsOf<ApplicationEventEnvelope>()([
    "event",
    "generation",
    "sequence",
  ]),
  applicationError: fieldsOf<ApplicationError>()(["code", "diagnostic", "recoverable"]),
  workspaceRequest: fieldsOf<WorkspaceRequest>()(["path"]),
  workspace: fieldsOf<Workspace>()(["path"]),
  newSessionRequest: fieldsOf<NewSessionRequest>()(["workspace"]),
  listSessionsRequest: fieldsOf<ListSessionsRequest>()(["cursor", "workspace"]),
  sessionWorkspaceRequest: fieldsOf<SessionWorkspaceRequest>()(["sessionId", "workspace"]),
  sessionRequest: fieldsOf<SessionRequest>()(["sessionId"]),
  promptRequest: fieldsOf<PromptRequest>()(["sessionId", "text"]),
  setSessionModeRequest: fieldsOf<SetSessionModeRequest>()(["modeId", "sessionId"]),
  setSessionModelRequest: fieldsOf<SetSessionModelRequest>()([
    "modelId",
    "reasoningEffort",
    "sessionId",
  ]),
  setSessionConfigRequest: fieldsOf<SetSessionConfigRequest>()([
    "configId",
    "sessionId",
    "value",
  ]),
  permissionResponseRequest: fieldsOf<PermissionResponseRequest>()([
    "decision",
    "interactionId",
  ]),
  elicitationResponseRequest: fieldsOf<ElicitationResponseRequest>()([
    "decision",
    "interactionId",
  ]),
  setupStatus: fieldsOf<SetupStatus>()(["failure", "runtimeAvailable"]),
  acknowledgement: fieldsOf<Acknowledgement>()(["acknowledged"]),
  promptResult: fieldsOf<PromptResult>()(["stopReason"]),
  runtimeSnapshot: fieldsOf<RuntimeSnapshot>()([
    "capabilities",
    "generation",
    "lastSequence",
    "state",
  ]),
  runtimeCapabilities: fieldsOf<RuntimeCapabilities>()([
    "agent",
    "authenticationMethods",
    "models",
    "protocolVersion",
    "sessions",
    "truncated",
  ]),
  runtimeAgent: fieldsOf<RuntimeAgent>()(["product", "version"]),
  runtimeSessionCapabilities: fieldsOf<RuntimeSessionCapabilities>()([
    "cancel",
    "close",
    "create",
    "list",
    "load",
    "prompt",
    "resume",
  ]),
  modelCatalog: fieldsOf<ModelCatalog>()(["availableModels", "currentModelId", "truncated"]),
  model: fieldsOf<Model>()([
    "agentType",
    "description",
    "modelId",
    "name",
    "reasoningEffort",
    "reasoningEfforts",
    "supportsReasoningEffort",
    "totalContextTokens",
    "truncated",
  ]),
  reasoningEffort: fieldsOf<ReasoningEffort>()([
    "description",
    "id",
    "isDefault",
    "label",
    "value",
  ]),
  session: fieldsOf<Session>()([
    "controls",
    "legacyConfigOptions",
    "models",
    "sessionId",
    "truncated",
  ]),
  legacyConfigOption: fieldsOf<LegacyConfigOption>()([
    "category",
    "description",
    "id",
    "label",
    "selected",
  ]),
  sessionControls: fieldsOf<SessionControls>()(["configOptions", "modes", "truncated"]),
  sessionModes: fieldsOf<SessionModes>()(["availableModes", "currentModeId", "truncated"]),
  sessionMode: fieldsOf<SessionMode>()(["description", "id", "name"]),
  sessionPage: fieldsOf<SessionPage>()(["nextCursor", "sessions", "truncated"]),
  sessionSummary: fieldsOf<SessionSummary>()(["sessionId", "title", "updatedAt", "workspace"]),
  planEntry: fieldsOf<PlanEntry>()(["description", "id", "status", "title"]),
  usage: fieldsOf<Usage>()([
    "cachedInputTokens",
    "contextWindowTokens",
    "inputTokens",
    "outputTokens",
    "totalTokens",
  ]),
  availableCommand: fieldsOf<AvailableCommand>()(["acceptsInput", "description", "name"]),
  configOption: fieldsOf<ConfigOption>()(["description", "id", "kind", "name"]),
  configChoice: fieldsOf<ConfigChoice>()(["description", "group", "name", "value"]),
} as const;

export const APPLICATION_VARIANT_FIELDS = {
  permissionScope: {
    tool: taggedFieldsOf<Extract<PermissionScope, { type: "tool" }>, "type">()(["toolName"]),
    command: taggedFieldsOf<Extract<PermissionScope, { type: "command" }>, "type">()([
      "command",
      "workingDirectory",
    ]),
    filesystem: taggedFieldsOf<
      Extract<PermissionScope, { type: "filesystem" }>,
      "type"
    >()(["operation", "path"]),
    network: taggedFieldsOf<Extract<PermissionScope, { type: "network" }>, "type">()([
      "destination",
    ]),
    other: taggedFieldsOf<Extract<PermissionScope, { type: "other" }>, "type">()([]),
  } satisfies TaggedVariantFieldMap<"type", PermissionScope>,
  elicitationControl: {
    text: taggedFieldsOf<Extract<ElicitationControl, { type: "text" }>, "type">()([
      "fieldId",
      "label",
      "placeholder",
      "sensitive",
    ]),
    confirmation: taggedFieldsOf<
      Extract<ElicitationControl, { type: "confirmation" }>,
      "type"
    >()(["fieldId", "label"]),
    choice: taggedFieldsOf<Extract<ElicitationControl, { type: "choice" }>, "type">()([
      "fieldId",
      "label",
      "multiple",
      "options",
      "truncated",
    ]),
    other: taggedFieldsOf<Extract<ElicitationControl, { type: "other" }>, "type">()([]),
  } satisfies TaggedVariantFieldMap<"type", ElicitationControl>,
  configValue: {
    select: taggedFieldsOf<Extract<ConfigValue, { type: "select" }>, "type">()(["value"]),
    boolean: taggedFieldsOf<Extract<ConfigValue, { type: "boolean" }>, "type">()(["value"]),
  } satisfies TaggedVariantFieldMap<"type", ConfigValue>,
  elicitationDecision: {
    accept: taggedFieldsOf<Extract<ElicitationDecision, { type: "accept" }>, "type">()([
      "value",
    ]),
    decline: taggedFieldsOf<Extract<ElicitationDecision, { type: "decline" }>, "type">()([]),
    cancel: taggedFieldsOf<Extract<ElicitationDecision, { type: "cancel" }>, "type">()([]),
  } satisfies TaggedVariantFieldMap<"type", ElicitationDecision>,
  elicitationValue: {
    string: taggedFieldsOf<Extract<ElicitationValue, { type: "string" }>, "type">()(["value"]),
    integer: taggedFieldsOf<Extract<ElicitationValue, { type: "integer" }>, "type">()(["value"]),
    number: taggedFieldsOf<Extract<ElicitationValue, { type: "number" }>, "type">()(["value"]),
    boolean: taggedFieldsOf<Extract<ElicitationValue, { type: "boolean" }>, "type">()(["value"]),
    string_array: taggedFieldsOf<
      Extract<ElicitationValue, { type: "string_array" }>,
      "type"
    >()(["value"]),
  } satisfies TaggedVariantFieldMap<"type", ElicitationValue>,
  optionalUpdate: {
    not_reported: taggedFieldsOf<
      Extract<OptionalUpdate, { state: "not_reported" }>,
      "state"
    >()([]),
    cleared: taggedFieldsOf<Extract<OptionalUpdate, { state: "cleared" }>, "state">()([]),
    value: taggedFieldsOf<Extract<OptionalUpdate, { state: "value" }>, "state">()(["value"]),
  } satisfies TaggedVariantFieldMap<"state", OptionalUpdate>,
  configOptionKind: {
    select: taggedFieldsOf<Extract<ConfigOptionKind, { type: "select" }>, "type">()([
      "currentValue",
      "options",
      "truncated",
    ]),
    boolean: taggedFieldsOf<Extract<ConfigOptionKind, { type: "boolean" }>, "type">()([
      "currentValue",
    ]),
  } satisfies TaggedVariantFieldMap<"type", ConfigOptionKind>,
} as const;

export const APPLICATION_ENUM_VALUES = {
  runtimeState: valuesOf<RuntimeState>()([
    "connecting",
    "authenticating",
    "ready",
    "working",
    "waiting_for_input",
    "failed",
    "disconnected",
  ]),
  sessionState: valuesOf<SessionState>()([
    "creating",
    "ready",
    "working",
    "waiting_for_input",
    "cancelling",
    "completed",
    "closed",
    "failed",
  ]),
  activityStatus: valuesOf<ActivityStatus>()([
    "pending",
    "running",
    "waiting_for_input",
    "completed",
    "failed",
    "cancelled",
  ]),
  toolCallKind: valuesOf<ToolCallKind>()([
    "tool",
    "terminal_command",
    "background_task",
    "subagent",
    "other",
  ]),
  permissionDecision: valuesOf<PermissionDecision>()([
    "allow_once",
    "allow_always",
    "deny_once",
    "deny_always",
    "cancel",
  ]),
  runtimeExtensionArea: valuesOf<RuntimeExtensionArea>()([
    "models",
    "settings",
    "sessions",
    "queue",
    "prompt_completion",
    "session",
    "announcements",
    "mcp",
    "malformed",
  ]),
  authenticationMethod: valuesOf<AuthenticationMethod>()([
    "cached_token",
    "grok_com",
    "xai_api_key",
    "grok",
    "other",
  ]),
  runtimeAgentProduct: valuesOf<RuntimeAgent["product"]>()(["grok_build", "other"]),
  promptStopReason: valuesOf<PromptResult["stopReason"]>()([
    "end_turn",
    "max_tokens",
    "max_turn_requests",
    "refusal",
    "cancelled",
    "other",
  ]),
  planEntryStatus: valuesOf<PlanEntry["status"]>()([
    "pending",
    "in_progress",
    "completed",
  ]),
  observedExtensionClassification: valuesOf<
    Extract<ApplicationEvent, { type: "extension_observed" }>["classification"]
  >()([
    "unknown",
    "invalid",
  ]),
  interactionKind: valuesOf<
    Extract<ApplicationEvent, { type: "interaction_resolved" }>["kind"]
  >()([
    "permission",
    "elicitation",
  ]),
  applicationErrorCode: valuesOf<ApplicationErrorCode>()([
    "executable_unavailable",
    "process_start_failed",
    "connection_failed",
    "request_timed_out",
    "initialize_failed",
    "unsupported_protocol",
    "unsupported_authentication",
    "authentication_failed",
    "runtime_stopped",
    "invalid_workspace",
    "invalid_request",
    "capability_unavailable",
    "protocol_request_failed",
    "malformed_response",
    "unknown_interaction",
    "decision_unavailable",
    "unexpected_response",
    "boundary_violation",
  ]),
} as const;
