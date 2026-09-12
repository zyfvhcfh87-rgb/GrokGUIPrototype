import {
  APPLICATION_COMMANDS,
  APPLICATION_EVENT_NAME,
  type Acknowledgement,
  type ApplicationEventEnvelope,
  type ElicitationResponseRequest,
  type ListSessionsRequest,
  type NewSessionRequest,
  type OpenExternalUrlRequest,
  type PermissionResponseRequest,
  type PresentationPreferences,
  type PromptRequest,
  type PromptResult,
  type RecentWorkspaceList,
  type RuntimeDiagnostics,
  type RuntimeSnapshot,
  type Session,
  type SessionPage,
  type SessionRequest,
  type SessionWorkspaceRequest,
  type SetSessionConfigRequest,
  type SetSessionModeRequest,
  type SetSessionModelRequest,
  type SetupStatus,
  type Workspace,
  type WorkspaceChanges,
  type WorkspaceRequest,
} from "./contract.ts";

export type Unlisten = () => void;

export type ApplicationTransport = {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(
    eventName: string,
    listener: (event: { payload: T }) => void,
  ): Promise<Unlisten>;
};

export function createApplicationBridge(transport: ApplicationTransport) {
  const invoke = <Result, Request>(command: string, request: Request) =>
    transport.invoke<Result>(command, { request });

  return {
    setupStatus: () => transport.invoke<SetupStatus>(APPLICATION_COMMANDS.setupStatus),
    pickWorkspace: () => transport.invoke<Workspace | null>(APPLICATION_COMMANDS.pickWorkspace),
    validateWorkspace: (request: WorkspaceRequest) =>
      invoke<Workspace, WorkspaceRequest>(APPLICATION_COMMANDS.validateWorkspace, request),
    listRecentWorkspaces: () =>
      transport.invoke<RecentWorkspaceList>(APPLICATION_COMMANDS.listRecentWorkspaces),
    removeRecentWorkspace: (request: WorkspaceRequest) =>
      invoke<RecentWorkspaceList, WorkspaceRequest>(
        APPLICATION_COMMANDS.removeRecentWorkspace,
        request,
      ),
    inspectWorkspaceChanges: (request: WorkspaceRequest) =>
      invoke<WorkspaceChanges, WorkspaceRequest>(
        APPLICATION_COMMANDS.inspectWorkspaceChanges,
        request,
      ),
    openExternalUrl: (request: OpenExternalUrlRequest) =>
      invoke<Acknowledgement, OpenExternalUrlRequest>(
        APPLICATION_COMMANDS.openExternalUrl,
        request,
      ),
    runtimeSnapshot: () =>
      transport.invoke<RuntimeSnapshot>(APPLICATION_COMMANDS.runtimeSnapshot),
    runtimeDiagnostics: () =>
      transport.invoke<RuntimeDiagnostics>(APPLICATION_COMMANDS.runtimeDiagnostics),
    startRuntime: () => transport.invoke<RuntimeSnapshot>(APPLICATION_COMMANDS.startRuntime),
    stopRuntime: () => transport.invoke<Acknowledgement>(APPLICATION_COMMANDS.stopRuntime),
    restartRuntime: () =>
      transport.invoke<RuntimeSnapshot>(APPLICATION_COMMANDS.restartRuntime),
    newSession: (request: NewSessionRequest) =>
      invoke<Session, NewSessionRequest>(APPLICATION_COMMANDS.newSession, request),
    listSessions: (request: ListSessionsRequest) =>
      invoke<SessionPage, ListSessionsRequest>(APPLICATION_COMMANDS.listSessions, request),
    loadSession: (request: SessionWorkspaceRequest) =>
      invoke<Session, SessionWorkspaceRequest>(APPLICATION_COMMANDS.loadSession, request),
    resumeSession: (request: SessionWorkspaceRequest) =>
      invoke<Session, SessionWorkspaceRequest>(APPLICATION_COMMANDS.resumeSession, request),
    closeSession: (request: SessionRequest) =>
      invoke<Acknowledgement, SessionRequest>(APPLICATION_COMMANDS.closeSession, request),
    sendPrompt: (request: PromptRequest) =>
      invoke<PromptResult, PromptRequest>(APPLICATION_COMMANDS.sendPrompt, request),
    cancelPrompt: (request: SessionRequest) =>
      invoke<Acknowledgement, SessionRequest>(APPLICATION_COMMANDS.cancelPrompt, request),
    setSessionMode: (request: SetSessionModeRequest) =>
      invoke<Acknowledgement, SetSessionModeRequest>(APPLICATION_COMMANDS.setSessionMode, request),
    setSessionModel: (request: SetSessionModelRequest) =>
      invoke<Acknowledgement, SetSessionModelRequest>(
        APPLICATION_COMMANDS.setSessionModel,
        request,
      ),
    setSessionConfig: (request: SetSessionConfigRequest) =>
      invoke<Acknowledgement, SetSessionConfigRequest>(
        APPLICATION_COMMANDS.setSessionConfig,
        request,
      ),
    respondToPermission: (request: PermissionResponseRequest) =>
      invoke<Acknowledgement, PermissionResponseRequest>(
        APPLICATION_COMMANDS.respondToPermission,
        request,
      ),
    respondToElicitation: (request: ElicitationResponseRequest) =>
      invoke<Acknowledgement, ElicitationResponseRequest>(
        APPLICATION_COMMANDS.respondToElicitation,
        request,
      ),
    getPresentationPreferences: () =>
      transport.invoke<PresentationPreferences>(
        APPLICATION_COMMANDS.getPresentationPreferences,
      ),
    setPresentationPreferences: (request: PresentationPreferences) =>
      invoke<PresentationPreferences, PresentationPreferences>(
        APPLICATION_COMMANDS.setPresentationPreferences,
        request,
      ),
    onEvent: (listener: (event: ApplicationEventEnvelope) => void) =>
      transport.listen<ApplicationEventEnvelope>(APPLICATION_EVENT_NAME, ({ payload }) => {
        listener(payload);
      }),
  } as const;
}

export type ApplicationBridge = ReturnType<typeof createApplicationBridge>;
