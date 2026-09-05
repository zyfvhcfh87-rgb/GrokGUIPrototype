use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as StdMutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use agent_client_protocol::schema::{
    MaybeUndefined, ProtocolVersion,
    v1::{
        AuthCapabilities, AuthenticateRequest, AvailableCommand, CancelNotification,
        ClientCapabilities, CloseSessionRequest, ContentBlock, CreateElicitationRequest,
        CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction,
        ElicitationCapabilities, ElicitationContentValue, ElicitationFormCapabilities,
        ElicitationMode, ElicitationPropertySchema, ElicitationScope, InitializeRequest,
        InitializeResponse, ListSessionsRequest, LoadSessionRequest, NewSessionRequest,
        PermissionOptionKind, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, ResumeSessionRequest, SelectedPermissionOutcome,
        SessionConfigKind, SessionConfigOption, SessionConfigOptionValue,
        SessionConfigSelectOptions, SessionModeState, SessionNotification, SessionUpdate,
        SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, TextContent, ToolCall,
        ToolCallStatus, ToolCallUpdate, ToolKind,
    },
};
#[cfg(not(windows))]
use agent_client_protocol::{AcpAgent, AcpAgentConfig};
use agent_client_protocol::{
    Agent, ConnectionTo, JsonRpcRequest, UntypedMessage, is_incoming_transport_closed,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use tokio::{
    sync::{Mutex, broadcast, mpsc, oneshot},
    task::JoinHandle,
};

#[cfg(windows)]
use crate::WindowsAcpProcess;
use crate::{
    ActivityStatus, ElicitationKind, GROK_STDIO_ARGS, GrokExtensionOutcome, GrokSessionResponse,
    ModelCatalog, PermissionKind, PlanEntry, PlanEntryStatus, RedactedDiagnostic,
    ResolvedGrokExecutable, RuntimeAvailableCommand, RuntimeEvent, RuntimeExtensionUpdate,
    RuntimeOptionalUpdate, RuntimeState, SessionMetadataKind, SessionState, ToolCallKind, Usage,
    normalize_grok_extension, normalize_grok_session_response, resolve_grok_executable,
};

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const COMMAND_CHANNEL_CAPACITY: usize = 64;
const EVENT_CHANNEL_CAPACITY: usize = 512;
const MAX_AUTH_METHODS: usize = 64;
const MAX_SAFE_LABEL_BYTES: usize = 256;
const MAX_SESSION_ID_BYTES: usize = 256;
const MAX_SESSION_TITLE_BYTES: usize = 512;
const MAX_TIMESTAMP_BYTES: usize = 128;
const MAX_CURSOR_BYTES: usize = 4 * 1_024;
const MAX_SESSIONS_PER_PAGE: usize = 512;
const MAX_PROMPT_BYTES: usize = 1024 * 1024;
const MAX_STREAM_CHUNK_BYTES: usize = 64 * 1024;
const MAX_PERMISSION_TEXT_BYTES: usize = 8 * 1024;
const MAX_ELICITATION_FIELDS: usize = 64;
const MAX_ELICITATION_VALUE_BYTES: usize = 16 * 1024;

/// A long-lived, application-facing Grok runtime.
///
/// Callers see lifecycle state, negotiated capabilities, domain commands, and
/// normalized events. ACP framing, request correlation, authentication wire
/// values, executable arguments, and process containment remain private here.
#[derive(Clone)]
pub struct GrokRuntime {
    shared: Arc<RuntimeShared>,
    target: RuntimeTarget,
    operation: Arc<Mutex<()>>,
    worker: Arc<Mutex<Option<RuntimeWorker>>>,
}

struct RuntimeShared {
    snapshot: RwLock<RuntimeSnapshot>,
    event_sender: broadcast::Sender<RuntimeEvent>,
    next_interaction_id: AtomicU64,
    pending_permissions: StdMutex<HashMap<String, PendingPermission>>,
    pending_elicitations: StdMutex<HashMap<String, PendingElicitation>>,
    interaction_gate: StdMutex<InteractionGateState>,
    tool_calls: StdMutex<HashMap<(String, String), ToolPresentation>>,
    active_prompts: RwLock<BTreeSet<String>>,
    session_controls: StdMutex<HashMap<String, RuntimeSessionControls>>,
    session_workspaces: StdMutex<HashMap<String, PathBuf>>,
}

#[derive(Default)]
struct InteractionGateState {
    runtime_stopping: bool,
    cancelled_sessions: BTreeSet<String>,
    closing_sessions: BTreeSet<String>,
    unavailable_sessions: BTreeSet<String>,
}

struct PendingPermission {
    session_id: String,
    options: BTreeMap<PermissionDecision, String>,
    response: oneshot::Sender<RequestPermissionResponse>,
}

struct PendingElicitation {
    session_id: Option<String>,
    kind: ElicitationKind,
    response: oneshot::Sender<CreateElicitationResponse>,
}

#[derive(Clone)]
struct ToolPresentation {
    title: String,
    kind: ToolCallKind,
    status: ActivityStatus,
}

struct RuntimeWorker {
    commands: mpsc::Sender<WorkerCommand>,
    task: JoinHandle<()>,
}

enum WorkerCommand {
    Execute {
        command: RuntimeCommand,
        response: oneshot::Sender<Result<RuntimeResponse, RuntimeError>>,
    },
    Stop {
        response: oneshot::Sender<()>,
    },
}

#[derive(Clone)]
struct RuntimeTarget {
    executable: PathBuf,
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
    auth_method: Option<String>,
    request_timeout: Duration,
}

#[cfg(feature = "test-support")]
#[derive(Clone, Debug)]
pub struct RuntimeTestTarget {
    executable: PathBuf,
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
    auth_method: Option<String>,
    request_timeout: Duration,
}

#[cfg(feature = "test-support")]
impl RuntimeTestTarget {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            arguments: Vec::new(),
            environment: Vec::new(),
            auth_method: None,
            request_timeout: Duration::from_secs(5),
        }
    }

    #[must_use]
    pub fn args(mut self, arguments: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.arguments = arguments.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.push((name.into(), value.into()));
        self
    }

    #[must_use]
    pub fn auth_method(mut self, method: impl Into<String>) -> Self {
        self.auth_method = Some(method.into());
        self
    }

    #[must_use]
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeSnapshot {
    pub state: RuntimeState,
    pub capabilities: Option<RuntimeCapabilities>,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            state: RuntimeState::Disconnected,
            capabilities: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeCapabilities {
    pub protocol_version: u16,
    pub agent: RuntimeAgent,
    pub authentication_methods: Vec<AuthenticationMethod>,
    pub sessions: RuntimeSessionCapabilities,
    pub models: Option<ModelCatalog>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeAgent {
    pub product: RuntimeAgentProduct,
    pub version: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAgentProduct {
    GrokBuild,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationMethod {
    CachedToken,
    GrokCom,
    XaiApiKey,
    Grok,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeSessionCapabilities {
    pub create: bool,
    pub prompt: bool,
    pub cancel: bool,
    pub list: bool,
    pub load: bool,
    pub resume: bool,
    pub close: bool,
}

/// Application domain commands accepted by the long-lived runtime.
///
/// These variants describe caller intent rather than ACP method names.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeCommand {
    NewSession {
        workspace: PathBuf,
    },
    ListSessions {
        workspace: Option<PathBuf>,
        cursor: Option<String>,
    },
    LoadSession {
        session_id: String,
        workspace: PathBuf,
    },
    ResumeSession {
        session_id: String,
        workspace: PathBuf,
    },
    CloseSession {
        session_id: String,
    },
    Prompt {
        session_id: String,
        text: String,
    },
    Cancel {
        session_id: String,
    },
    SetSessionMode {
        session_id: String,
        mode_id: String,
    },
    SetSessionModel {
        session_id: String,
        model_id: String,
        reasoning_effort: Option<String>,
    },
    SetSessionConfig {
        session_id: String,
        config_id: String,
        value: RuntimeConfigValue,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum RuntimeResponse {
    Session(RuntimeSession),
    Sessions(RuntimeSessionPage),
    PromptCompleted {
        stop_reason: RuntimePromptStopReason,
    },
    Acknowledged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePromptStopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    AllowOnce,
    AllowAlways,
    DenyOnce,
    DenyAlways,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ElicitationDecision {
    Accept {
        content: BTreeMap<String, ElicitationValue>,
    },
    Decline,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ElicitationValue {
    String(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    StringArray(Vec<String>),
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonRpcRequest)]
#[request(method = "session/set_model", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
struct SetSessionModelRequest {
    session_id: String,
    model_id: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    meta: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeSession {
    pub session_id: String,
    pub metadata: GrokSessionResponse,
    pub controls: RuntimeSessionControls,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RuntimeSessionControls {
    pub modes: Option<RuntimeModes>,
    pub config_options: Vec<RuntimeConfigOption>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeModes {
    pub current_mode_id: String,
    pub available_modes: Vec<RuntimeMode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeMode {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: RuntimeConfigKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeConfigKind {
    Select {
        current_value: String,
        options: Vec<RuntimeConfigChoice>,
    },
    Boolean {
        current_value: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeConfigChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
    pub group: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum RuntimeConfigValue {
    Select(String),
    Boolean(bool),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeSessionPage {
    pub sessions: Vec<RuntimeSessionSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeSessionSummary {
    pub session_id: String,
    pub workspace: PathBuf,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq, Serialize)]
#[error("Grok runtime operation failed ({code})")]
pub struct RuntimeError {
    pub code: RuntimeErrorCode,
    pub diagnostic: RedactedDiagnostic,
    pub recoverable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeErrorCode {
    ExecutableUnavailable,
    ProcessStartFailed,
    ConnectionFailed,
    RequestTimedOut,
    InitializeFailed,
    UnsupportedProtocol,
    UnsupportedAuthentication,
    AuthenticationFailed,
    RuntimeStopped,
    InvalidWorkspace,
    InvalidRequest,
    CapabilityUnavailable,
    ProtocolRequestFailed,
    MalformedResponse,
    UnknownInteraction,
    DecisionUnavailable,
}

impl std::fmt::Display for RuntimeErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Self::ExecutableUnavailable => "executable_unavailable",
            Self::ProcessStartFailed => "process_start_failed",
            Self::ConnectionFailed => "connection_failed",
            Self::RequestTimedOut => "request_timed_out",
            Self::InitializeFailed => "initialize_failed",
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::UnsupportedAuthentication => "unsupported_authentication",
            Self::AuthenticationFailed => "authentication_failed",
            Self::RuntimeStopped => "runtime_stopped",
            Self::InvalidWorkspace => "invalid_workspace",
            Self::InvalidRequest => "invalid_request",
            Self::CapabilityUnavailable => "capability_unavailable",
            Self::ProtocolRequestFailed => "protocol_request_failed",
            Self::MalformedResponse => "malformed_response",
            Self::UnknownInteraction => "unknown_interaction",
            Self::DecisionUnavailable => "decision_unavailable",
        };
        formatter.write_str(code)
    }
}

impl GrokRuntime {
    /// Discover the installed Grok executable without exposing its resolved
    /// path or launch arguments to application callers.
    pub fn discover(explicit_path: Option<&Path>) -> Result<Self, RuntimeError> {
        let executable = resolve_grok_executable(explicit_path).map_err(|error| RuntimeError {
            code: RuntimeErrorCode::ExecutableUnavailable,
            diagnostic: RedactedDiagnostic::new(error.to_string()),
            recoverable: true,
        })?;

        Ok(Self::from_resolved_executable(executable))
    }

    /// Build a runtime from an executable that has already passed the shared
    /// discovery and canonicalization boundary.
    #[must_use]
    pub fn from_resolved_executable(executable: ResolvedGrokExecutable) -> Self {
        Self::from_target(RuntimeTarget {
            executable: executable.into_path(),
            arguments: GROK_STDIO_ARGS.iter().map(ToString::to_string).collect(),
            environment: Vec::new(),
            auth_method: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn for_test(target: RuntimeTestTarget) -> Self {
        Self::from_target(RuntimeTarget {
            executable: target.executable,
            arguments: target.arguments,
            environment: target.environment,
            auth_method: target.auth_method,
            request_timeout: target.request_timeout,
        })
    }

    fn from_target(target: RuntimeTarget) -> Self {
        let (event_sender, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            shared: Arc::new(RuntimeShared {
                snapshot: RwLock::new(RuntimeSnapshot::default()),
                event_sender,
                next_interaction_id: AtomicU64::new(1),
                pending_permissions: StdMutex::new(HashMap::new()),
                pending_elicitations: StdMutex::new(HashMap::new()),
                interaction_gate: StdMutex::new(InteractionGateState::default()),
                tool_calls: StdMutex::new(HashMap::new()),
                active_prompts: RwLock::new(BTreeSet::new()),
                session_controls: StdMutex::new(HashMap::new()),
                session_workspaces: StdMutex::new(HashMap::new()),
            }),
            target,
            operation: Arc::new(Mutex::new(())),
            worker: Arc::new(Mutex::new(None)),
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.shared.event_sender.subscribe()
    }

    #[must_use]
    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.shared
            .snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub async fn start(&self) -> Result<RuntimeSnapshot, RuntimeError> {
        let _operation = self.operation.lock().await;

        {
            let mut worker = self.worker.lock().await;
            if let Some(current) = worker.as_ref()
                && !current.task.is_finished()
            {
                return Ok(self.snapshot());
            }
            if let Some(finished) = worker.take() {
                let _ = finished.task.await;
            }
        }
        self.start_inner().await
    }

    pub async fn stop(&self) -> Result<(), RuntimeError> {
        let _operation = self.operation.lock().await;
        self.stop_inner().await;
        Ok(())
    }

    pub async fn restart(&self) -> Result<RuntimeSnapshot, RuntimeError> {
        let _operation = self.operation.lock().await;
        self.stop_inner().await;
        self.start_inner().await
    }

    pub async fn execute(&self, command: RuntimeCommand) -> Result<RuntimeResponse, RuntimeError> {
        let commands = {
            let worker = self.worker.lock().await;
            let Some(worker) = worker.as_ref().filter(|worker| !worker.task.is_finished()) else {
                return Err(runtime_error(
                    RuntimeErrorCode::RuntimeStopped,
                    "runtime is not connected",
                    true,
                ));
            };
            worker.commands.clone()
        };
        let (response_tx, response_rx) = oneshot::channel();
        commands
            .send(WorkerCommand::Execute {
                command,
                response: response_tx,
            })
            .await
            .map_err(|_| {
                runtime_error(
                    RuntimeErrorCode::RuntimeStopped,
                    "runtime stopped before accepting the command",
                    true,
                )
            })?;
        response_rx.await.map_err(|_| {
            runtime_error(
                RuntimeErrorCode::RuntimeStopped,
                "runtime stopped before completing the command",
                true,
            )
        })?
    }

    pub async fn respond_permission(
        &self,
        interaction_id: &str,
        decision: PermissionDecision,
    ) -> Result<(), RuntimeError> {
        let mut pending = self
            .shared
            .pending_permissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(advertised) = pending.get(interaction_id) else {
            return Err(runtime_error(
                RuntimeErrorCode::UnknownInteraction,
                "permission request is no longer pending",
                false,
            ));
        };
        let response = if decision == PermissionDecision::Cancel {
            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
        } else {
            let Some(option_id) = advertised.options.get(&decision) else {
                return Err(runtime_error(
                    RuntimeErrorCode::DecisionUnavailable,
                    "permission decision was not advertised by the runtime",
                    false,
                ));
            };
            RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(option_id.clone()),
            ))
        };
        let pending = pending
            .remove(interaction_id)
            .expect("advertised permission must remain pending while locked");
        pending.response.send(response).map_err(|_| {
            runtime_error(
                RuntimeErrorCode::UnknownInteraction,
                "permission request expired before the decision was delivered",
                false,
            )
        })
    }

    pub async fn respond_elicitation(
        &self,
        interaction_id: &str,
        decision: ElicitationDecision,
    ) -> Result<(), RuntimeError> {
        let mut pending = self
            .shared
            .pending_elicitations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(advertised) = pending.get(interaction_id) else {
            return Err(runtime_error(
                RuntimeErrorCode::UnknownInteraction,
                "elicitation request is no longer pending",
                false,
            ));
        };
        validate_elicitation_decision(&advertised.kind, &decision)?;
        let response = normalize_elicitation_response(decision)?;
        let pending = pending
            .remove(interaction_id)
            .expect("advertised elicitation must remain pending while locked");
        pending.response.send(response).map_err(|_| {
            runtime_error(
                RuntimeErrorCode::UnknownInteraction,
                "elicitation request expired before the decision was delivered",
                false,
            )
        })
    }

    async fn start_inner(&self) -> Result<RuntimeSnapshot, RuntimeError> {
        self.shared.allow_runtime_interactions();
        self.shared.transition(RuntimeState::Connecting);
        let (commands, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let (ready_tx, ready_rx) = oneshot::channel();
        let shared = Arc::clone(&self.shared);
        let target = self.target.clone();
        let task = tokio::spawn(async move {
            run_worker(target, shared, command_rx, ready_tx).await;
        });
        *self.worker.lock().await = Some(RuntimeWorker { commands, task });
        let result = match ready_rx.await {
            Ok(result) => result,
            Err(_) => Err(runtime_error(
                RuntimeErrorCode::ConnectionFailed,
                "runtime process ended before initialization completed",
                true,
            )),
        };
        if result.is_err() {
            self.shutdown_current_worker().await;
        }
        result
    }

    async fn stop_inner(&self) {
        self.shutdown_current_worker().await;
        if self.snapshot().state != RuntimeState::Disconnected {
            self.shared.transition(RuntimeState::Disconnected);
        }
    }

    async fn shutdown_current_worker(&self) {
        let Some(worker) = self.worker.lock().await.take() else {
            return;
        };
        shutdown_worker(worker).await;
    }
}

impl RuntimeShared {
    fn allow_runtime_interactions(&self) {
        let mut gate = self
            .interaction_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gate.runtime_stopping = false;
        gate.cancelled_sessions.clear();
        gate.closing_sessions.clear();
        gate.unavailable_sessions.clear();
    }

    fn transition(&self, state: RuntimeState) {
        let changed = {
            let mut snapshot = self
                .snapshot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if snapshot.state == state {
                false
            } else {
                snapshot.state = state;
                true
            }
        };
        if changed {
            self.emit(RuntimeEvent::RuntimeStateChanged { state });
        }
    }

    fn ready(&self, capabilities: RuntimeCapabilities) -> RuntimeSnapshot {
        let snapshot = {
            let mut snapshot = self
                .snapshot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            snapshot.capabilities = Some(capabilities);
            snapshot.state = RuntimeState::Ready;
            snapshot.clone()
        };
        self.emit(RuntimeEvent::RuntimeStateChanged {
            state: RuntimeState::Ready,
        });
        snapshot
    }

    fn fail(&self, error: &RuntimeError) {
        self.transition(RuntimeState::Failed);
        self.emit(RuntimeEvent::RuntimeFailed {
            diagnostic: error.diagnostic.clone(),
            recoverable: error.recoverable,
        });
    }

    fn session_state(&self, session_id: &str, state: SessionState) {
        self.emit(RuntimeEvent::SessionStateChanged {
            session_id: session_id.to_owned(),
            state,
        });
    }

    fn remember_session_workspace(&self, session_id: &str, workspace: PathBuf) {
        self.session_workspaces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.to_owned(), workspace);
    }

    fn require_session_workspace(
        &self,
        session_id: &str,
        workspace: &Path,
    ) -> Result<(), RuntimeError> {
        let known = self
            .session_workspaces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match known.get(session_id) {
            Some(known) if !same_workspace(known, workspace) => Err(runtime_error(
                RuntimeErrorCode::InvalidWorkspace,
                "session does not belong to the selected workspace",
                true,
            )),
            _ => Ok(()),
        }
    }

    fn remember_session_controls(&self, session: &RuntimeSession) {
        self.session_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session.session_id.clone(), session.controls.clone());
    }

    fn update_current_mode(&self, session_id: &str, mode_id: String) {
        if let Some(modes) = self
            .session_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(session_id)
            .and_then(|controls| controls.modes.as_mut())
        {
            modes.current_mode_id = mode_id;
        }
    }

    fn update_config_options(&self, session_id: &str, options: Vec<RuntimeConfigOption>) {
        if let Some(controls) = self
            .session_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(session_id)
        {
            controls.config_options = options;
        }
    }

    fn update_current_model(&self, model_id: String) {
        if let Some(models) = self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .capabilities
            .as_mut()
            .and_then(|capabilities| capabilities.models.as_mut())
        {
            models.current_model_id = model_id;
        }
    }

    fn emit(&self, event: RuntimeEvent) {
        let _ = self.event_sender.send(event);
    }

    fn next_interaction_id(&self, prefix: &str) -> String {
        let sequence = self.next_interaction_id.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{sequence}")
    }

    fn register_permission(
        &self,
        session_id: String,
        interaction_id: String,
        options: BTreeMap<PermissionDecision, String>,
        event: RuntimeEvent,
    ) -> Option<oneshot::Receiver<RequestPermissionResponse>> {
        let gate = self
            .interaction_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if gate.runtime_stopping
            || gate.cancelled_sessions.contains(&session_id)
            || gate.closing_sessions.contains(&session_id)
            || gate.unavailable_sessions.contains(&session_id)
        {
            return None;
        }
        let (response_tx, response_rx) = oneshot::channel();
        self.pending_permissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                interaction_id,
                PendingPermission {
                    session_id: session_id.clone(),
                    options,
                    response: response_tx,
                },
            );
        self.transition(RuntimeState::WaitingForInput);
        self.session_state(&session_id, SessionState::WaitingForInput);
        self.emit(event);
        Some(response_rx)
    }

    fn register_elicitation(
        &self,
        session_id: Option<String>,
        interaction_id: String,
        kind: ElicitationKind,
        event: RuntimeEvent,
    ) -> Option<oneshot::Receiver<CreateElicitationResponse>> {
        let gate = self
            .interaction_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if gate.runtime_stopping
            || session_id.as_ref().is_some_and(|session_id| {
                gate.cancelled_sessions.contains(session_id)
                    || gate.closing_sessions.contains(session_id)
                    || gate.unavailable_sessions.contains(session_id)
            })
        {
            return None;
        }
        let (response_tx, response_rx) = oneshot::channel();
        self.pending_elicitations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                interaction_id,
                PendingElicitation {
                    session_id: session_id.clone(),
                    kind,
                    response: response_tx,
                },
            );
        self.transition(RuntimeState::WaitingForInput);
        if let Some(session_id) = &session_id {
            self.session_state(session_id, SessionState::WaitingForInput);
        }
        self.emit(event);
        Some(response_rx)
    }

    fn begin_prompt(&self, session_id: &str) -> Result<(), RuntimeError> {
        let mut active = self
            .active_prompts
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !active.is_empty() {
            return Err(runtime_error(
                RuntimeErrorCode::InvalidRequest,
                "another prompt is already active",
                true,
            ));
        }
        {
            let mut gate = self
                .interaction_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if gate.runtime_stopping {
                return Err(runtime_error(
                    RuntimeErrorCode::RuntimeStopped,
                    "runtime is stopping",
                    true,
                ));
            }
            if gate.closing_sessions.contains(session_id)
                || gate.unavailable_sessions.contains(session_id)
            {
                return Err(runtime_error(
                    RuntimeErrorCode::InvalidRequest,
                    "session must be loaded or resumed before prompting",
                    true,
                ));
            }
            gate.cancelled_sessions.remove(session_id);
        }
        active.insert(session_id.to_owned());
        drop(active);
        self.transition(RuntimeState::Working);
        self.session_state(session_id, SessionState::Working);
        Ok(())
    }

    fn end_prompt(&self, session_id: &str) {
        let none_active = {
            let mut active = self
                .active_prompts
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            active.remove(session_id);
            active.is_empty()
        };
        let lifecycle_blocked = {
            let gate = self
                .interaction_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            gate.closing_sessions.contains(session_id)
                || gate.unavailable_sessions.contains(session_id)
        };
        if !lifecycle_blocked {
            self.session_state(session_id, SessionState::Ready);
        }
        if none_active {
            self.transition(RuntimeState::Ready);
        }
    }

    fn handle_session_notification(&self, notification: SessionNotification) {
        let session_id = notification.session_id.to_string();
        match notification.update {
            SessionUpdate::UserMessageChunk(chunk) => {
                if let Some(text) = content_text(chunk.content) {
                    self.emit(RuntimeEvent::UserMessageChunkReceived {
                        session_id,
                        message_id: chunk.message_id.map(|id| id.to_string()),
                        text,
                    });
                }
            }
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let Some(text) = content_text(chunk.content) {
                    self.emit(RuntimeEvent::MessageChunkReceived {
                        session_id,
                        message_id: chunk.message_id.map(|id| id.to_string()),
                        text,
                    });
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                if let Some(text) = content_text(chunk.content) {
                    self.emit(RuntimeEvent::ThoughtChunkReceived {
                        session_id,
                        thought_id: chunk.message_id.map(|id| id.to_string()),
                        text,
                    });
                }
            }
            SessionUpdate::ToolCall(call) => self.handle_tool_call(&session_id, call),
            SessionUpdate::ToolCallUpdate(update) => {
                self.handle_tool_call_update(&session_id, update);
            }
            SessionUpdate::Plan(plan) => {
                let entries = plan
                    .entries
                    .into_iter()
                    .take(256)
                    .enumerate()
                    .map(|(index, entry)| PlanEntry {
                        id: format!("standard-plan-{index}"),
                        title: bounded_text(&entry.content, MAX_PERMISSION_TEXT_BYTES),
                        description: None,
                        status: match entry.status {
                            agent_client_protocol::schema::v1::PlanEntryStatus::Pending => {
                                PlanEntryStatus::Pending
                            }
                            agent_client_protocol::schema::v1::PlanEntryStatus::InProgress => {
                                PlanEntryStatus::InProgress
                            }
                            agent_client_protocol::schema::v1::PlanEntryStatus::Completed => {
                                PlanEntryStatus::Completed
                            }
                            _ => PlanEntryStatus::Pending,
                        },
                    })
                    .collect();
                self.emit(RuntimeEvent::PlanChanged {
                    session_id,
                    entries,
                });
            }
            SessionUpdate::UsageUpdate(usage) => {
                self.emit(RuntimeEvent::UsageChanged {
                    session_id,
                    usage: Usage {
                        total_tokens: Some(usage.used),
                        context_window_tokens: Some(usage.size),
                        ..Usage::default()
                    },
                });
            }
            SessionUpdate::AvailableCommandsUpdate(update) => {
                match normalize_available_commands(update.available_commands) {
                    Ok(commands) => self.emit(RuntimeEvent::AvailableCommandsChanged {
                        session_id,
                        commands,
                    }),
                    Err(error) => self.report_recoverable_update_error(error),
                }
            }
            SessionUpdate::CurrentModeUpdate(update) => {
                match validate_response_token(update.current_mode_id.0.as_ref()) {
                    Ok(current_mode_id) => {
                        self.update_current_mode(&session_id, current_mode_id.clone());
                        self.emit(RuntimeEvent::SessionModeChanged {
                            session_id,
                            current_mode_id,
                        });
                    }
                    Err(error) => self.report_recoverable_update_error(error),
                }
            }
            SessionUpdate::ConfigOptionUpdate(update) => {
                match normalize_config_options(&update.config_options) {
                    Ok(config_options) => {
                        self.update_config_options(&session_id, config_options.clone());
                        self.emit(RuntimeEvent::SessionConfigOptionsChanged {
                            session_id,
                            config_options,
                        });
                    }
                    Err(error) => self.report_recoverable_update_error(error),
                }
            }
            SessionUpdate::SessionInfoUpdate(update) => {
                match normalize_session_info(update.title, update.updated_at) {
                    Ok((title, updated_at)) => self.emit(RuntimeEvent::SessionInfoChanged {
                        session_id,
                        title,
                        updated_at,
                    }),
                    Err(error) => self.report_recoverable_update_error(error),
                }
            }
            _ => self.emit_session_metadata(session_id, SessionMetadataKind::Other),
        }
    }

    fn emit_session_metadata(&self, session_id: String, kind: SessionMetadataKind) {
        self.emit(RuntimeEvent::SessionMetadataChanged { session_id, kind });
    }

    fn report_recoverable_update_error(&self, error: RuntimeError) {
        self.emit(RuntimeEvent::RuntimeFailed {
            diagnostic: error.diagnostic,
            recoverable: true,
        });
    }

    fn handle_tool_call(&self, session_id: &str, call: ToolCall) {
        let call_id = call.tool_call_id.to_string();
        let presentation = ToolPresentation {
            title: bounded_text(&call.title, MAX_PERMISSION_TEXT_BYTES),
            kind: normalize_tool_kind(call.kind),
            status: normalize_tool_status(call.status),
        };
        self.tool_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                (session_id.to_owned(), call_id.clone()),
                presentation.clone(),
            );
        self.emit(RuntimeEvent::ToolCallChanged {
            session_id: session_id.to_owned(),
            call_id,
            title: presentation.title,
            kind: presentation.kind,
            status: presentation.status,
            detail: None,
        });
    }

    fn handle_tool_call_update(&self, session_id: &str, update: ToolCallUpdate) {
        let call_id = update.tool_call_id.to_string();
        let key = (session_id.to_owned(), call_id.clone());
        let presentation = {
            let mut tools = self
                .tool_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let current = tools.entry(key).or_insert_with(|| ToolPresentation {
                title: "Tool activity".to_owned(),
                kind: ToolCallKind::Other,
                status: ActivityStatus::Pending,
            });
            if let Some(title) = update.fields.title {
                current.title = bounded_text(&title, MAX_PERMISSION_TEXT_BYTES);
            }
            if let Some(kind) = update.fields.kind {
                current.kind = normalize_tool_kind(kind);
            }
            if let Some(status) = update.fields.status {
                current.status = normalize_tool_status(status);
            }
            current.clone()
        };
        self.emit(RuntimeEvent::ToolCallChanged {
            session_id: session_id.to_owned(),
            call_id,
            title: presentation.title,
            kind: presentation.kind,
            status: presentation.status,
            detail: None,
        });
    }

    async fn handle_permission_request(
        &self,
        request: RequestPermissionRequest,
    ) -> RequestPermissionResponse {
        let interaction_id = self.next_interaction_id("permission");
        let Some((event, options)) = normalize_permission_request(&request, interaction_id.clone())
        else {
            self.emit(RuntimeEvent::RuntimeFailed {
                diagnostic: RedactedDiagnostic::new("malformed permission request was denied"),
                recoverable: true,
            });
            return RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
        };
        let session_id = request.session_id.to_string();
        let Some(response_rx) =
            self.register_permission(session_id.clone(), interaction_id, options, event)
        else {
            return RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
        };
        let response = response_rx.await.unwrap_or_else(|_| {
            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
        });
        self.restore_after_interaction(&session_id);
        response
    }

    async fn handle_elicitation_request(
        &self,
        request: CreateElicitationRequest,
    ) -> CreateElicitationResponse {
        let interaction_id = self.next_interaction_id("elicitation");
        let Some(session_id) = elicitation_session_id(request.scope()) else {
            return self.deny_unsupported_elicitation();
        };
        let Some(prompt) =
            validate_exact_interaction_text(&request.message, MAX_PERMISSION_TEXT_BYTES)
        else {
            return self.deny_unsupported_elicitation();
        };
        let Some(kind) = normalize_elicitation_kind(&request.mode) else {
            return self.deny_unsupported_elicitation();
        };
        let event = RuntimeEvent::ElicitationRequested {
            session_id: session_id.clone(),
            interaction_id: interaction_id.clone(),
            prompt,
            kind: kind.clone(),
        };
        let Some(response_rx) =
            self.register_elicitation(session_id.clone(), interaction_id, kind, event)
        else {
            return CreateElicitationResponse::new(ElicitationAction::Cancel);
        };
        let response = response_rx
            .await
            .unwrap_or_else(|_| CreateElicitationResponse::new(ElicitationAction::Cancel));
        if let Some(session_id) = &session_id {
            self.restore_after_interaction(session_id);
        } else {
            self.transition(RuntimeState::Authenticating);
        }
        response
    }

    fn deny_unsupported_elicitation(&self) -> CreateElicitationResponse {
        self.emit(RuntimeEvent::RuntimeFailed {
            diagnostic: RedactedDiagnostic::new("unsupported elicitation request was declined"),
            recoverable: true,
        });
        CreateElicitationResponse::new(ElicitationAction::Decline)
    }

    fn restore_after_interaction(&self, session_id: &str) {
        let interactions_cancelled = {
            let gate = self
                .interaction_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            gate.runtime_stopping
                || gate.cancelled_sessions.contains(session_id)
                || gate.closing_sessions.contains(session_id)
                || gate.unavailable_sessions.contains(session_id)
        };
        if interactions_cancelled {
            return;
        }
        let active = self
            .active_prompts
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(session_id);
        if active {
            self.transition(RuntimeState::Working);
            self.session_state(session_id, SessionState::Working);
        } else {
            self.transition(RuntimeState::Ready);
            self.session_state(session_id, SessionState::Ready);
        }
    }

    fn restore_after_failed_close(&self, session_id: &str) {
        {
            let mut gate = self
                .interaction_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            gate.cancelled_sessions.remove(session_id);
            gate.closing_sessions.remove(session_id);
        }
        let (target_active, none_active) = {
            let active = self
                .active_prompts
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (active.contains(session_id), active.is_empty())
        };
        if target_active {
            self.session_state(session_id, SessionState::Working);
            self.transition(RuntimeState::Working);
        } else {
            self.session_state(session_id, SessionState::Ready);
            if none_active {
                self.transition(RuntimeState::Ready);
            }
        }
    }

    fn mark_session_unavailable(&self, session_id: &str, state: SessionState) {
        let mut gate = self
            .interaction_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gate.cancelled_sessions.remove(session_id);
        gate.closing_sessions.remove(session_id);
        gate.unavailable_sessions.insert(session_id.to_owned());
        drop(gate);
        self.session_state(session_id, state);
    }

    fn begin_session_close(&self, session_id: &str) -> Result<(), RuntimeError> {
        let mut gate = self
            .interaction_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if gate.closing_sessions.contains(session_id)
            || gate.unavailable_sessions.contains(session_id)
        {
            return Err(runtime_error(
                RuntimeErrorCode::InvalidRequest,
                "session must be loaded or resumed before this operation",
                true,
            ));
        }
        gate.closing_sessions.insert(session_id.to_owned());
        Ok(())
    }

    fn activate_session(&self, session_id: &str) {
        let mut gate = self
            .interaction_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gate.cancelled_sessions.remove(session_id);
        gate.closing_sessions.remove(session_id);
        gate.unavailable_sessions.remove(session_id);
    }

    fn handle_extension_notification(&self, notification: UntypedMessage) {
        let (method, params) = notification.into_parts();
        match normalize_grok_extension(&method, params) {
            Ok(GrokExtensionOutcome::RuntimeEvent(event)) => self.emit(event),
            Ok(GrokExtensionOutcome::ModelsUpdated(models)) => {
                if let Some(capabilities) = self
                    .snapshot
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .capabilities
                    .as_mut()
                {
                    capabilities.models = Some(models.clone());
                }
                self.emit(RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Models(models),
                });
            }
            Ok(GrokExtensionOutcome::SettingsUpdated(settings)) => {
                self.emit(RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Settings(settings),
                });
            }
            Ok(GrokExtensionOutcome::SessionsChanged(sessions)) => {
                self.emit(RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Sessions(sessions),
                });
            }
            Ok(GrokExtensionOutcome::QueueChanged(queue)) => {
                self.emit(RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Queue(queue),
                });
            }
            Ok(GrokExtensionOutcome::PromptCompleted(completion)) => {
                self.emit(RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::PromptCompletion(completion),
                });
            }
            Ok(GrokExtensionOutcome::SessionUpdated(session)) => {
                self.emit(RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Session(session),
                });
            }
            Ok(GrokExtensionOutcome::AnnouncementsUpdated(announcements)) => {
                self.emit(RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Announcements(announcements),
                });
            }
            Ok(GrokExtensionOutcome::McpChanged(mcp)) => {
                self.emit(RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Mcp(mcp),
                });
            }
            Ok(GrokExtensionOutcome::Malformed(malformed)) => {
                self.emit(RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Malformed(malformed),
                });
            }
            Err(_) => {
                self.emit(RuntimeEvent::ExtensionMethodObserved {
                    session_id: None,
                    method: crate::ExtensionMethod::new("x.ai/invalid_extension")
                        .expect("fixed invalid extension marker must remain valid"),
                });
            }
        }
    }

    async fn cancel_runtime_activity(&self, connection: &ConnectionTo<Agent>) {
        let sessions: Vec<_> = self
            .active_prompts
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect();
        for session_id in sessions {
            let _ = connection.send_notification(CancelNotification::new(session_id));
        }
        self.cancel_pending_interactions().await;
    }

    async fn cancel_session_interactions(&self, session_id: &str) {
        let mut gate = self
            .interaction_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gate.cancelled_sessions.insert(session_id.to_owned());
        self.session_state(session_id, SessionState::Cancelling);
        let permissions = {
            let mut pending = self
                .pending_permissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let ids = pending
                .iter()
                .filter(|(_, pending)| pending.session_id == session_id)
                .map(|(interaction_id, _)| interaction_id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|interaction_id| pending.remove(&interaction_id))
                .collect::<Vec<_>>()
        };
        let elicitations = {
            let mut pending = self
                .pending_elicitations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let ids = pending
                .iter()
                .filter(|(_, pending)| pending.session_id.as_deref() == Some(session_id))
                .map(|(interaction_id, _)| interaction_id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|interaction_id| pending.remove(&interaction_id))
                .collect::<Vec<_>>()
        };
        self.emit(RuntimeEvent::InteractionsCleared {
            session_id: Some(session_id.to_owned()),
        });
        drop(gate);
        for pending in permissions {
            let _ = pending.response.send(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ));
        }
        for pending in elicitations {
            let _ = pending
                .response
                .send(CreateElicitationResponse::new(ElicitationAction::Cancel));
        }
    }

    async fn cancel_pending_interactions(&self) {
        let mut gate = self
            .interaction_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gate.runtime_stopping = true;
        let permissions = self
            .pending_permissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|(_, pending)| pending)
            .collect::<Vec<_>>();
        let elicitations = self
            .pending_elicitations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|(_, pending)| pending)
            .collect::<Vec<_>>();
        self.emit(RuntimeEvent::InteractionsCleared { session_id: None });
        drop(gate);
        for pending in permissions {
            let _ = pending.response.send(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ));
        }
        for pending in elicitations {
            let _ = pending
                .response
                .send(CreateElicitationResponse::new(ElicitationAction::Cancel));
        }
    }
}

fn content_text(content: ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(bounded_text(&text.text, MAX_STREAM_CHUNK_BYTES)),
        _ => None,
    }
}

fn normalize_available_commands(
    commands: Vec<AvailableCommand>,
) -> Result<Vec<RuntimeAvailableCommand>, RuntimeError> {
    if commands.len() > 256 {
        return Err(malformed_response(
            "runtime advertised too many available commands",
        ));
    }
    commands
        .into_iter()
        .map(|command| {
            Ok(RuntimeAvailableCommand {
                name: validate_response_token(&command.name)?,
                description: validate_response_description(&command.description)?,
                accepts_input: command.input.is_some(),
            })
        })
        .collect()
}

fn normalize_session_info(
    title: MaybeUndefined<String>,
    updated_at: MaybeUndefined<String>,
) -> Result<(RuntimeOptionalUpdate<String>, RuntimeOptionalUpdate<String>), RuntimeError> {
    Ok((
        normalize_optional_update(title, MAX_SESSION_TITLE_BYTES, "session title")?,
        normalize_optional_update(updated_at, MAX_TIMESTAMP_BYTES, "session timestamp")?,
    ))
}

fn normalize_optional_update(
    value: MaybeUndefined<String>,
    maximum: usize,
    label: &str,
) -> Result<RuntimeOptionalUpdate<String>, RuntimeError> {
    match value {
        MaybeUndefined::Undefined => Ok(RuntimeOptionalUpdate::NotReported),
        MaybeUndefined::Null => Ok(RuntimeOptionalUpdate::Cleared),
        MaybeUndefined::Value(value)
            if value.len() <= maximum && !value.chars().any(char::is_control) =>
        {
            Ok(RuntimeOptionalUpdate::Value(value))
        }
        MaybeUndefined::Value(_) => Err(malformed_response(format!(
            "runtime advertised an invalid {label}"
        ))),
    }
}

fn bounded_text(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut boundary = maximum;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

fn normalize_tool_kind(kind: ToolKind) -> ToolCallKind {
    match kind {
        ToolKind::Execute => ToolCallKind::TerminalCommand,
        _ => ToolCallKind::Tool,
    }
}

fn normalize_tool_status(status: ToolCallStatus) -> ActivityStatus {
    match status {
        ToolCallStatus::Pending => ActivityStatus::Pending,
        ToolCallStatus::InProgress => ActivityStatus::Running,
        ToolCallStatus::Completed => ActivityStatus::Completed,
        ToolCallStatus::Failed => ActivityStatus::Failed,
        _ => ActivityStatus::Pending,
    }
}

fn normalize_permission_request(
    request: &RequestPermissionRequest,
    interaction_id: String,
) -> Option<(RuntimeEvent, BTreeMap<PermissionDecision, String>)> {
    let session_id =
        validate_exact_interaction_text(request.session_id.0.as_ref(), MAX_SESSION_ID_BYTES)?;
    let title = request
        .tool_call
        .fields
        .title
        .as_deref()
        .and_then(|title| validate_exact_interaction_text(title, MAX_PERMISSION_TEXT_BYTES))
        .unwrap_or_else(|| "Permission requested".to_owned());
    let kind = permission_kind(&request.tool_call)?;
    let mut options = BTreeMap::new();
    let mut available_decisions = Vec::new();
    for option in request.options.iter().take(16) {
        let decision = match option.kind {
            PermissionOptionKind::AllowOnce => PermissionDecision::AllowOnce,
            PermissionOptionKind::AllowAlways => PermissionDecision::AllowAlways,
            PermissionOptionKind::RejectOnce => PermissionDecision::DenyOnce,
            PermissionOptionKind::RejectAlways => PermissionDecision::DenyAlways,
            _ => continue,
        };
        let option_id = validate_exact_interaction_text(option.option_id.0.as_ref(), 256)?;
        if options.insert(decision, option_id).is_some() {
            return None;
        }
        available_decisions.push(decision);
    }
    if options.is_empty() {
        return None;
    }

    Some((
        RuntimeEvent::PermissionRequested {
            session_id,
            interaction_id,
            title,
            consequence: permission_consequence(&request.tool_call),
            kind,
            available_decisions,
        },
        options,
    ))
}

fn permission_kind(update: &ToolCallUpdate) -> Option<PermissionKind> {
    let raw = update.fields.raw_input.as_ref().and_then(Value::as_object);
    let command = raw
        .and_then(|raw| raw.get("command"))
        .and_then(Value::as_str)
        .and_then(|command| validate_exact_interaction_text(command, MAX_PERMISSION_TEXT_BYTES));
    let working_directory = raw
        .and_then(|raw| raw.get("cwd"))
        .and_then(Value::as_str)
        .and_then(validate_absolute_interaction_path);
    if matches!(update.fields.kind, Some(ToolKind::Execute))
        && let Some(command) = command
        && let Some(working_directory) = working_directory
    {
        return Some(PermissionKind::Command {
            command,
            working_directory: Some(working_directory),
        });
    }

    let path = raw
        .and_then(|raw| raw.get("affectedPath").or_else(|| raw.get("path")))
        .and_then(Value::as_str)
        .and_then(validate_absolute_interaction_path)
        .or_else(|| {
            update
                .fields
                .locations
                .as_ref()
                .and_then(|locations| locations.first())
                .map(|location| location.path.clone())
                .filter(|path| path.is_absolute())
        });
    if let Some(path) = path {
        let operation = match update.fields.kind {
            Some(ToolKind::Read) => "read",
            Some(ToolKind::Edit) => "edit",
            Some(ToolKind::Delete) => "delete",
            Some(ToolKind::Move) => "move",
            _ => return None,
        };
        return Some(PermissionKind::Filesystem {
            operation: operation.to_owned(),
            path,
        });
    }

    None
}

fn permission_consequence(update: &ToolCallUpdate) -> Option<String> {
    let path = update
        .fields
        .raw_input
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|raw| raw.get("affectedPath"))
        .and_then(Value::as_str)
        .and_then(validate_absolute_interaction_path)?;
    Some(format!("May affect {}", path.display()))
}

fn validate_absolute_interaction_path(value: &str) -> Option<PathBuf> {
    validate_exact_interaction_text(value, MAX_PERMISSION_TEXT_BYTES).and_then(|value| {
        let path = PathBuf::from(value);
        path.is_absolute().then_some(path)
    })
}

fn validate_exact_interaction_text(value: &str, maximum: usize) -> Option<String> {
    (!value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

fn elicitation_session_id(scope: &ElicitationScope) -> Option<Option<String>> {
    match scope {
        ElicitationScope::Session(scope) => {
            validate_exact_interaction_text(scope.session_id.0.as_ref(), MAX_SESSION_ID_BYTES)
                .map(Some)
        }
        ElicitationScope::Request(_) => Some(None),
        _ => None,
    }
}

fn normalize_elicitation_kind(mode: &ElicitationMode) -> Option<ElicitationKind> {
    let ElicitationMode::Form(form) = mode else {
        return None;
    };
    if form.requested_schema.properties.len() != 1 {
        return None;
    }
    let (field_id, schema) = form.requested_schema.properties.first_key_value()?;
    let field_id = validate_exact_interaction_text(field_id, 256)?;
    Some(match schema {
        ElicitationPropertySchema::String(schema) => {
            let label = schema
                .title
                .as_deref()
                .and_then(|value| validate_exact_interaction_text(value, 512));
            let options = schema.enum_values.clone().or_else(|| {
                schema
                    .one_of
                    .as_ref()
                    .map(|options| options.iter().map(|option| option.value.clone()).collect())
            });
            if let Some(options) = options {
                if options.is_empty()
                    || options.len() > 64
                    || !options
                        .iter()
                        .all(|option| validate_exact_interaction_text(option, 512).is_some())
                {
                    return None;
                }
                ElicitationKind::Choice {
                    field_id,
                    label,
                    options,
                    multiple: false,
                }
            } else {
                ElicitationKind::Text {
                    field_id,
                    label,
                    placeholder: schema
                        .default
                        .as_deref()
                        .and_then(|value| validate_exact_interaction_text(value, 512)),
                    sensitive: false,
                }
            }
        }
        ElicitationPropertySchema::Boolean(schema) => ElicitationKind::Confirmation {
            field_id,
            label: schema
                .title
                .as_deref()
                .and_then(|value| validate_exact_interaction_text(value, 512)),
        },
        _ => return None,
    })
}

fn normalize_elicitation_response(
    decision: ElicitationDecision,
) -> Result<CreateElicitationResponse, RuntimeError> {
    let action = match decision {
        ElicitationDecision::Accept { content } => {
            if content.len() > MAX_ELICITATION_FIELDS {
                return Err(runtime_error(
                    RuntimeErrorCode::InvalidRequest,
                    "elicitation response contained too many fields",
                    false,
                ));
            }
            let mut normalized = BTreeMap::new();
            for (key, value) in content {
                let Some(key) = validate_exact_interaction_text(&key, 256) else {
                    return Err(runtime_error(
                        RuntimeErrorCode::InvalidRequest,
                        "elicitation response contained an invalid field name",
                        false,
                    ));
                };
                normalized.insert(key, normalize_elicitation_value(value)?);
            }
            ElicitationAction::Accept(ElicitationAcceptAction::new().content(normalized))
        }
        ElicitationDecision::Decline => ElicitationAction::Decline,
        ElicitationDecision::Cancel => ElicitationAction::Cancel,
    };
    Ok(CreateElicitationResponse::new(action))
}

fn validate_elicitation_decision(
    kind: &ElicitationKind,
    decision: &ElicitationDecision,
) -> Result<(), RuntimeError> {
    let ElicitationDecision::Accept { content } = decision else {
        return Ok(());
    };
    if content.len() != 1 {
        return Err(invalid_elicitation_decision());
    }
    let valid = match kind {
        ElicitationKind::Text { field_id, .. } => {
            matches!(content.get(field_id), Some(ElicitationValue::String(_)))
        }
        ElicitationKind::Confirmation { field_id, .. } => {
            matches!(content.get(field_id), Some(ElicitationValue::Boolean(_)))
        }
        ElicitationKind::Choice {
            field_id,
            options,
            multiple: false,
            ..
        } => matches!(
            content.get(field_id),
            Some(ElicitationValue::String(value)) if options.contains(value)
        ),
        ElicitationKind::Choice {
            field_id,
            options,
            multiple: true,
            ..
        } => matches!(
            content.get(field_id),
            Some(ElicitationValue::StringArray(values))
                if !values.is_empty() && values.iter().all(|value| options.contains(value))
        ),
        ElicitationKind::Other => false,
    };
    valid.then_some(()).ok_or_else(invalid_elicitation_decision)
}

fn invalid_elicitation_decision() -> RuntimeError {
    runtime_error(
        RuntimeErrorCode::DecisionUnavailable,
        "elicitation response did not match the advertised form",
        false,
    )
}

fn normalize_elicitation_value(
    value: ElicitationValue,
) -> Result<ElicitationContentValue, RuntimeError> {
    match value {
        ElicitationValue::String(value) => {
            validate_elicitation_string(value).map(ElicitationContentValue::String)
        }
        ElicitationValue::Integer(value) => Ok(ElicitationContentValue::Integer(value)),
        ElicitationValue::Number(value) if value.is_finite() => {
            Ok(ElicitationContentValue::Number(value))
        }
        ElicitationValue::Number(_) => Err(invalid_elicitation_value()),
        ElicitationValue::Boolean(value) => Ok(ElicitationContentValue::Boolean(value)),
        ElicitationValue::StringArray(values) if values.len() <= 64 => values
            .into_iter()
            .map(validate_elicitation_string)
            .collect::<Result<Vec<_>, _>>()
            .map(ElicitationContentValue::StringArray),
        ElicitationValue::StringArray(_) => Err(invalid_elicitation_value()),
    }
}

fn validate_elicitation_string(value: String) -> Result<String, RuntimeError> {
    if value.len() <= MAX_ELICITATION_VALUE_BYTES && !value.chars().any(char::is_control) {
        Ok(value)
    } else {
        Err(invalid_elicitation_value())
    }
}

fn invalid_elicitation_value() -> RuntimeError {
    runtime_error(
        RuntimeErrorCode::InvalidRequest,
        "elicitation response contained an invalid value",
        false,
    )
}

async fn run_worker(
    target: RuntimeTarget,
    shared: Arc<RuntimeShared>,
    command_rx: mpsc::Receiver<WorkerCommand>,
    ready_tx: oneshot::Sender<Result<RuntimeSnapshot, RuntimeError>>,
) {
    #[cfg(windows)]
    let agent = WindowsAcpProcess::new(target.executable.clone())
        .args(target.arguments.clone())
        .envs(target.environment.clone());
    #[cfg(not(windows))]
    let agent = AcpAgent::new(
        AcpAgentConfig::new(target.executable.clone())
            .args(target.arguments.clone())
            .envs(target.environment.clone()),
    );

    let shared_for_connection = Arc::clone(&shared);
    let target_for_connection = target.clone();
    let shared_for_updates = Arc::clone(&shared);
    let shared_for_permissions = Arc::clone(&shared);
    let shared_for_elicitations = Arc::clone(&shared);
    let shared_for_extensions = Arc::clone(&shared);
    let connection = agent_client_protocol::Client
        .builder()
        .name("grok-build-gui-runtime")
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                shared_for_updates.handle_session_notification(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let response = shared_for_permissions
                    .handle_permission_request(request)
                    .await;
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: CreateElicitationRequest, responder, _connection| {
                let response = shared_for_elicitations
                    .handle_elicitation_request(request)
                    .await;
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: UntypedMessage, _connection| {
                shared_for_extensions.handle_extension_notification(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| async move {
            let initialized =
                initialize_runtime(&connection, &target_for_connection, &shared_for_connection)
                    .await;
            let snapshot = match initialized {
                Ok(capabilities) => shared_for_connection.ready(capabilities),
                Err(error) => {
                    let _ = ready_tx.send(Err(error.clone()));
                    return Ok::<_, agent_client_protocol::Error>(Err(error));
                }
            };
            let _ = ready_tx.send(Ok(snapshot));
            let capabilities = shared_for_connection
                .snapshot
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .capabilities
                .clone()
                .expect("ready snapshot must contain capabilities");
            run_command_loop(
                connection.clone(),
                command_rx,
                Arc::clone(&shared_for_connection),
                capabilities,
                target_for_connection.request_timeout,
            )
            .await;
            Ok::<_, agent_client_protocol::Error>(Ok(()))
        })
        .await;

    shared.cancel_pending_interactions().await;
    match connection {
        Ok(Ok(())) => shared.transition(RuntimeState::Disconnected),
        Ok(Err(error)) => shared.fail(&error),
        Err(_) => shared.fail(&runtime_error(
            RuntimeErrorCode::ConnectionFailed,
            "ACP connection failed",
            true,
        )),
    }
}

async fn shutdown_worker(mut worker: RuntimeWorker) {
    if worker.task.is_finished() {
        let _ = worker.task.await;
        return;
    }

    let graceful = async {
        let (response_tx, response_rx) = oneshot::channel();
        if worker
            .commands
            .send(WorkerCommand::Stop {
                response: response_tx,
            })
            .await
            .is_ok()
        {
            let _ = response_rx.await;
        }
        let _ = (&mut worker.task).await;
    };
    if tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, graceful)
        .await
        .is_err()
    {
        worker.task.abort();
        let _ = worker.task.await;
    }
}

async fn initialize_runtime(
    connection: &ConnectionTo<Agent>,
    target: &RuntimeTarget,
    shared: &RuntimeShared,
) -> Result<RuntimeCapabilities, RuntimeError> {
    let initialize = tokio::time::timeout(
        target.request_timeout,
        connection
            .send_request(
                InitializeRequest::new(ProtocolVersion::V1)
                    .client_capabilities(client_capabilities()),
            )
            .block_task(),
    )
    .await
    .map_err(|_| {
        runtime_error(
            RuntimeErrorCode::RequestTimedOut,
            "runtime initialization timed out",
            true,
        )
    })?
    .map_err(|_| {
        runtime_error(
            RuntimeErrorCode::InitializeFailed,
            "runtime initialization request failed",
            true,
        )
    })?;

    if initialize.protocol_version != ProtocolVersion::V1 {
        return Err(runtime_error(
            RuntimeErrorCode::UnsupportedProtocol,
            "runtime did not negotiate ACP protocol v1",
            false,
        ));
    }

    if !initialize.auth_methods.is_empty() {
        let auth_method = choose_auth_method(&initialize, target.auth_method.as_deref())
            .ok_or_else(|| {
                runtime_error(
                    RuntimeErrorCode::UnsupportedAuthentication,
                    "runtime advertised no supported authentication method",
                    true,
                )
            })?;
        shared.transition(RuntimeState::Authenticating);
        let mut meta = Map::new();
        meta.insert("headless".to_owned(), Value::Bool(true));
        tokio::time::timeout(
            target.request_timeout,
            connection
                .send_request(AuthenticateRequest::new(auth_method).meta(meta))
                .block_task(),
        )
        .await
        .map_err(|_| {
            runtime_error(
                RuntimeErrorCode::RequestTimedOut,
                "runtime authentication timed out",
                true,
            )
        })?
        .map_err(|_| {
            runtime_error(
                RuntimeErrorCode::AuthenticationFailed,
                "runtime authentication request failed",
                true,
            )
        })?;
    }

    Ok(normalize_capabilities(&initialize))
}

async fn run_command_loop(
    connection: ConnectionTo<Agent>,
    mut commands: mpsc::Receiver<WorkerCommand>,
    shared: Arc<RuntimeShared>,
    capabilities: RuntimeCapabilities,
    request_timeout: Duration,
) {
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    WorkerCommand::Execute { command, response }
                        if matches!(command, RuntimeCommand::Prompt { .. }) =>
                    {
                        let connection = connection.clone();
                        let shared = Arc::clone(&shared);
                        let capabilities = capabilities.clone();
                        tasks.spawn(async move {
                            let result = execute_command(
                                &connection,
                                &shared,
                                &capabilities,
                                request_timeout,
                                command,
                            )
                            .await;
                            let _ = response.send(result);
                        });
                    }
                    WorkerCommand::Execute { command, response } => {
                        let result = execute_command(
                            &connection,
                            &shared,
                            &capabilities,
                            request_timeout,
                            command,
                        )
                        .await;
                        let _ = response.send(result);
                    }
                    WorkerCommand::Stop { response } => {
                        shared.cancel_runtime_activity(&connection).await;
                        tasks.abort_all();
                        while tasks.join_next().await.is_some() {}
                        let _ = response.send(());
                        break;
                    }
                }
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

async fn execute_command(
    connection: &ConnectionTo<Agent>,
    shared: &RuntimeShared,
    capabilities: &RuntimeCapabilities,
    request_timeout: Duration,
    command: RuntimeCommand,
) -> Result<RuntimeResponse, RuntimeError> {
    match command {
        RuntimeCommand::NewSession { workspace } => {
            let workspace = canonicalize_workspace(workspace)?;
            let response = timeout_request(
                request_timeout,
                connection
                    .send_request(NewSessionRequest::new(workspace.clone()))
                    .block_task(),
            )
            .await?;
            let session_id = validate_session_id(response.session_id.to_string())?;
            let session = normalize_runtime_session(
                session_id.clone(),
                response.modes.as_ref(),
                response.config_options.as_deref(),
                normalize_serializable_session_response(&response),
            )?;
            shared.remember_session_workspace(&session_id, workspace);
            shared.activate_session(&session_id);
            shared.remember_session_controls(&session);
            shared.session_state(&session_id, SessionState::Ready);
            Ok(RuntimeResponse::Session(session))
        }
        RuntimeCommand::ListSessions { workspace, cursor } => {
            require_capability(capabilities.sessions.list, "session listing")?;
            let workspace = workspace.map(canonicalize_workspace).transpose()?;
            let cursor = validate_cursor(cursor)?;
            let request = ListSessionsRequest::new()
                .cwd(workspace.clone())
                .cursor(cursor);
            let response = timeout_request(
                request_timeout,
                connection.send_request(request).block_task(),
            )
            .await?;
            let mut page = match normalize_session_page(response)? {
                RuntimeResponse::Sessions(page) => page,
                other => return Ok(other),
            };
            for session in &page.sessions {
                shared.remember_session_workspace(&session.session_id, session.workspace.clone());
            }
            if let Some(workspace) = workspace.as_ref() {
                page.sessions
                    .retain(|session| same_workspace(&session.workspace, workspace));
            }
            Ok(RuntimeResponse::Sessions(page))
        }
        RuntimeCommand::LoadSession {
            session_id,
            workspace,
        } => {
            require_capability(capabilities.sessions.load, "session loading")?;
            let session_id = validate_session_id(session_id)?;
            let workspace = canonicalize_workspace(workspace)?;
            shared.require_session_workspace(&session_id, &workspace)?;
            shared.emit(RuntimeEvent::SessionActivated {
                session_id: session_id.clone(),
            });
            let result = async {
                let response = timeout_request(
                    request_timeout,
                    connection
                        .send_request(LoadSessionRequest::new(
                            session_id.clone(),
                            workspace.clone(),
                        ))
                        .block_task(),
                )
                .await?;
                let session = normalize_runtime_session(
                    session_id.clone(),
                    response.modes.as_ref(),
                    response.config_options.as_deref(),
                    normalize_serializable_session_response(&response),
                )?;
                shared.remember_session_workspace(&session_id, workspace);
                shared.activate_session(&session_id);
                shared.remember_session_controls(&session);
                Ok(RuntimeResponse::Session(session))
            }
            .await;
            shared.session_state(
                &session_id,
                if result.is_ok() {
                    SessionState::Ready
                } else {
                    SessionState::Failed
                },
            );
            result
        }
        RuntimeCommand::ResumeSession {
            session_id,
            workspace,
        } => {
            require_capability(capabilities.sessions.resume, "session resuming")?;
            let session_id = validate_session_id(session_id)?;
            let workspace = canonicalize_workspace(workspace)?;
            shared.require_session_workspace(&session_id, &workspace)?;
            shared.emit(RuntimeEvent::SessionActivated {
                session_id: session_id.clone(),
            });
            let result = async {
                let response = timeout_request(
                    request_timeout,
                    connection
                        .send_request(ResumeSessionRequest::new(
                            session_id.clone(),
                            workspace.clone(),
                        ))
                        .block_task(),
                )
                .await?;
                let session = normalize_runtime_session(
                    session_id.clone(),
                    response.modes.as_ref(),
                    response.config_options.as_deref(),
                    normalize_serializable_session_response(&response),
                )?;
                shared.remember_session_workspace(&session_id, workspace);
                shared.activate_session(&session_id);
                shared.remember_session_controls(&session);
                Ok(RuntimeResponse::Session(session))
            }
            .await;
            shared.session_state(
                &session_id,
                if result.is_ok() {
                    SessionState::Ready
                } else {
                    SessionState::Failed
                },
            );
            result
        }
        RuntimeCommand::CloseSession { session_id } => {
            require_capability(capabilities.sessions.close, "session closing")?;
            let session_id = validate_session_id(session_id)?;
            shared.begin_session_close(&session_id)?;
            shared.cancel_session_interactions(&session_id).await;
            let _ = connection.send_notification(CancelNotification::new(session_id.clone()));
            match tokio::time::timeout(
                request_timeout,
                connection
                    .send_request(CloseSessionRequest::new(session_id.clone()))
                    .block_task(),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(error)) if !is_incoming_transport_closed(&error) => {
                    shared.restore_after_failed_close(&session_id);
                    return Err(runtime_error(
                        RuntimeErrorCode::ProtocolRequestFailed,
                        "runtime rejected session closing",
                        true,
                    ));
                }
                Ok(Err(_)) => {
                    shared.mark_session_unavailable(&session_id, SessionState::Failed);
                    return Err(runtime_error(
                        RuntimeErrorCode::ConnectionFailed,
                        "session close outcome is unknown after connection loss",
                        true,
                    ));
                }
                Err(_) => {
                    shared.mark_session_unavailable(&session_id, SessionState::Failed);
                    return Err(runtime_error(
                        RuntimeErrorCode::RequestTimedOut,
                        "session close outcome is unknown after timeout",
                        true,
                    ));
                }
            }
            shared.mark_session_unavailable(&session_id, SessionState::Closed);
            shared
                .session_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&session_id);
            Ok(RuntimeResponse::Acknowledged)
        }
        RuntimeCommand::SetSessionMode {
            session_id,
            mode_id,
        } => {
            let session_id = validate_session_id(session_id)?;
            let mode_id = validate_response_token(&mode_id)?;
            require_advertised_mode(shared, &session_id, &mode_id)?;
            timeout_request(
                request_timeout,
                connection
                    .send_request(SetSessionModeRequest::new(
                        session_id.clone(),
                        mode_id.clone(),
                    ))
                    .block_task(),
            )
            .await?;
            shared.update_current_mode(&session_id, mode_id);
            Ok(RuntimeResponse::Acknowledged)
        }
        RuntimeCommand::SetSessionModel {
            session_id,
            model_id,
            reasoning_effort,
        } => {
            let session_id = validate_session_id(session_id)?;
            let model_id = validate_response_token(&model_id)?;
            let reasoning_effort = reasoning_effort
                .map(|effort| validate_response_token(&effort))
                .transpose()?;
            require_advertised_model(capabilities, &model_id, reasoning_effort.as_deref())?;
            let meta = reasoning_effort.as_ref().map(|effort| {
                Map::from_iter([("reasoningEffort".to_owned(), Value::String(effort.clone()))])
            });
            timeout_request(
                request_timeout,
                connection
                    .send_request(SetSessionModelRequest {
                        session_id,
                        model_id: model_id.clone(),
                        meta,
                    })
                    .block_task(),
            )
            .await?;
            shared.update_current_model(model_id);
            Ok(RuntimeResponse::Acknowledged)
        }
        RuntimeCommand::SetSessionConfig {
            session_id,
            config_id,
            value,
        } => {
            let session_id = validate_session_id(session_id)?;
            let config_id = validate_response_token(&config_id)?;
            require_advertised_config(shared, &session_id, &config_id, &value)?;
            let wire_value = match value {
                RuntimeConfigValue::Select(value) => {
                    SessionConfigOptionValue::value_id(validate_response_token(&value)?)
                }
                RuntimeConfigValue::Boolean(value) => SessionConfigOptionValue::boolean(value),
            };
            let response = timeout_request(
                request_timeout,
                connection
                    .send_request(SetSessionConfigOptionRequest::new(
                        session_id.clone(),
                        config_id,
                        wire_value,
                    ))
                    .block_task(),
            )
            .await?;
            let config_options = response
                .config_options
                .iter()
                .map(normalize_config_option)
                .collect::<Result<Vec<_>, _>>()?;
            shared.update_config_options(&session_id, config_options);
            Ok(RuntimeResponse::Acknowledged)
        }
        RuntimeCommand::Prompt { session_id, text } => {
            let session_id = validate_session_id(session_id)?;
            let text = validate_prompt(text)?;
            shared.begin_prompt(&session_id)?;
            let response = connection
                .send_request(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new(text))],
                ))
                .block_task()
                .await
                .map_err(|_| {
                    runtime_error(
                        RuntimeErrorCode::ProtocolRequestFailed,
                        "runtime rejected the prompt",
                        true,
                    )
                });
            shared.end_prompt(&session_id);
            let response = response?;
            Ok(RuntimeResponse::PromptCompleted {
                stop_reason: normalize_stop_reason(response.stop_reason),
            })
        }
        RuntimeCommand::Cancel { session_id } => {
            let session_id = validate_session_id(session_id)?;
            shared.cancel_session_interactions(&session_id).await;
            connection
                .send_notification(CancelNotification::new(session_id))
                .map_err(|_| {
                    runtime_error(
                        RuntimeErrorCode::ProtocolRequestFailed,
                        "runtime rejected cancellation",
                        true,
                    )
                })?;
            Ok(RuntimeResponse::Acknowledged)
        }
    }
}

async fn timeout_request<T>(
    timeout: Duration,
    request: impl std::future::Future<Output = Result<T, agent_client_protocol::Error>>,
) -> Result<T, RuntimeError> {
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| {
            runtime_error(
                RuntimeErrorCode::RequestTimedOut,
                "runtime command timed out",
                true,
            )
        })?
        .map_err(|_| {
            runtime_error(
                RuntimeErrorCode::ProtocolRequestFailed,
                "runtime rejected the command",
                true,
            )
        })
}

fn normalize_serializable_session_response<T: Serialize>(response: &T) -> GrokSessionResponse {
    serde_json::to_value(response)
        .map(normalize_grok_session_response)
        .unwrap_or_default()
}

fn normalize_runtime_session(
    session_id: String,
    modes: Option<&SessionModeState>,
    config_options: Option<&[SessionConfigOption]>,
    metadata: GrokSessionResponse,
) -> Result<RuntimeSession, RuntimeError> {
    let modes = modes.map(normalize_modes).transpose()?;
    let config_options = normalize_config_options(config_options.unwrap_or_default())?;
    Ok(RuntimeSession {
        session_id,
        metadata,
        controls: RuntimeSessionControls {
            modes,
            config_options,
        },
    })
}

fn normalize_config_options(
    config_options: &[SessionConfigOption],
) -> Result<Vec<RuntimeConfigOption>, RuntimeError> {
    if config_options.len() > 64 {
        return Err(malformed_response(
            "runtime advertised too many configuration options",
        ));
    }
    config_options.iter().map(normalize_config_option).collect()
}

fn normalize_modes(modes: &SessionModeState) -> Result<RuntimeModes, RuntimeError> {
    if modes.available_modes.len() > 64 {
        return Err(malformed_response(
            "runtime advertised too many session modes",
        ));
    }
    let current_mode_id = validate_response_token(modes.current_mode_id.0.as_ref())?;
    let mut seen = BTreeSet::new();
    let mut available_modes = Vec::with_capacity(modes.available_modes.len());
    for mode in &modes.available_modes {
        let id = validate_response_token(mode.id.0.as_ref())?;
        if !seen.insert(id.clone()) {
            return Err(malformed_response(
                "runtime advertised duplicate session modes",
            ));
        }
        available_modes.push(RuntimeMode {
            id,
            name: validate_response_label(&mode.name)?,
            description: mode
                .description
                .as_deref()
                .map(validate_response_description)
                .transpose()?,
        });
    }
    if !seen.contains(&current_mode_id) {
        return Err(malformed_response(
            "runtime current mode was absent from advertised modes",
        ));
    }
    Ok(RuntimeModes {
        current_mode_id,
        available_modes,
    })
}

fn normalize_config_option(
    option: &SessionConfigOption,
) -> Result<RuntimeConfigOption, RuntimeError> {
    let kind = match &option.kind {
        SessionConfigKind::Boolean(boolean) => RuntimeConfigKind::Boolean {
            current_value: boolean.current_value,
        },
        SessionConfigKind::Select(select) => {
            let mut choices = Vec::new();
            match &select.options {
                SessionConfigSelectOptions::Ungrouped(options) => {
                    for option in options.iter().take(256) {
                        choices.push(normalize_config_choice(option, None)?);
                    }
                    if options.len() > 256 {
                        return Err(malformed_response(
                            "runtime config option advertised too many choices",
                        ));
                    }
                }
                SessionConfigSelectOptions::Grouped(groups) => {
                    for group in groups.iter().take(64) {
                        let group_name = validate_response_label(&group.name)?;
                        for option in &group.options {
                            if choices.len() == 256 {
                                return Err(malformed_response(
                                    "runtime config option advertised too many choices",
                                ));
                            }
                            choices
                                .push(normalize_config_choice(option, Some(group_name.clone()))?);
                        }
                    }
                    if groups.len() > 64 {
                        return Err(malformed_response(
                            "runtime config option advertised too many groups",
                        ));
                    }
                }
                _ => {
                    return Err(malformed_response(
                        "runtime config option used an unsupported choice shape",
                    ));
                }
            }
            let current_value = validate_response_token(select.current_value.0.as_ref())?;
            if !choices.iter().any(|choice| choice.value == current_value) {
                return Err(malformed_response(
                    "runtime config current value was not advertised",
                ));
            }
            RuntimeConfigKind::Select {
                current_value,
                options: choices,
            }
        }
        _ => {
            return Err(malformed_response(
                "runtime advertised an unsupported config option",
            ));
        }
    };
    Ok(RuntimeConfigOption {
        id: validate_response_token(option.id.0.as_ref())?,
        name: validate_response_label(&option.name)?,
        description: option
            .description
            .as_deref()
            .map(validate_response_description)
            .transpose()?,
        kind,
    })
}

fn normalize_config_choice(
    option: &agent_client_protocol::schema::v1::SessionConfigSelectOption,
    group: Option<String>,
) -> Result<RuntimeConfigChoice, RuntimeError> {
    Ok(RuntimeConfigChoice {
        value: validate_response_token(option.value.0.as_ref())?,
        name: validate_response_label(&option.name)?,
        description: option
            .description
            .as_deref()
            .map(validate_response_description)
            .transpose()?,
        group,
    })
}

fn validate_response_token(value: &str) -> Result<String, RuntimeError> {
    validate_text(value.to_owned(), 256, "runtime response identifier")
}

fn validate_response_label(value: &str) -> Result<String, RuntimeError> {
    validate_text(value.to_owned(), 512, "runtime response label")
}

fn validate_response_description(value: &str) -> Result<String, RuntimeError> {
    if value.len() <= 4 * 1024 {
        Ok(value.to_owned())
    } else {
        Err(malformed_response(
            "runtime response description exceeded its size limit",
        ))
    }
}

fn normalize_session_page(
    response: agent_client_protocol::schema::v1::ListSessionsResponse,
) -> Result<RuntimeResponse, RuntimeError> {
    if response.sessions.len() > MAX_SESSIONS_PER_PAGE {
        return Err(malformed_response("session page exceeded its item limit"));
    }
    let next_cursor = validate_cursor(response.next_cursor)?;
    let mut seen = BTreeSet::new();
    let mut sessions = Vec::with_capacity(response.sessions.len());
    for session in response.sessions {
        let session_id = validate_session_id(session.session_id.to_string())?;
        if !seen.insert(session_id.clone()) {
            return Err(malformed_response(
                "session page contained duplicate identities",
            ));
        }
        if !session.cwd.is_absolute() {
            return Err(malformed_response(
                "session page contained a relative workspace",
            ));
        }
        sessions.push(RuntimeSessionSummary {
            session_id,
            workspace: session.cwd,
            title: validate_optional_text(session.title, MAX_SESSION_TITLE_BYTES)?,
            updated_at: validate_optional_text(session.updated_at, MAX_TIMESTAMP_BYTES)?,
        });
    }
    Ok(RuntimeResponse::Sessions(RuntimeSessionPage {
        sessions,
        next_cursor,
    }))
}

fn same_workspace(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    let equal = |left: &Path, right: &Path| {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    };
    #[cfg(not(windows))]
    let equal = |left: &Path, right: &Path| left == right;

    if equal(left, right) {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => equal(&left, &right),
        _ => false,
    }
}

fn canonicalize_workspace(workspace: PathBuf) -> Result<PathBuf, RuntimeError> {
    if !workspace.is_dir() {
        return Err(runtime_error(
            RuntimeErrorCode::InvalidWorkspace,
            "workspace is not an existing directory",
            true,
        ));
    }
    std::fs::canonicalize(workspace).map_err(|_| {
        runtime_error(
            RuntimeErrorCode::InvalidWorkspace,
            "workspace could not be canonicalized",
            true,
        )
    })
}

fn validate_session_id(session_id: String) -> Result<String, RuntimeError> {
    validate_text(session_id, MAX_SESSION_ID_BYTES, "session identifier")
}

fn validate_prompt(text: String) -> Result<String, RuntimeError> {
    if text.is_empty() || text.len() > MAX_PROMPT_BYTES {
        return Err(runtime_error(
            RuntimeErrorCode::InvalidRequest,
            "prompt text was empty or exceeded its size limit",
            false,
        ));
    }
    Ok(text)
}

fn normalize_stop_reason(reason: StopReason) -> RuntimePromptStopReason {
    match reason {
        StopReason::EndTurn => RuntimePromptStopReason::EndTurn,
        StopReason::MaxTokens => RuntimePromptStopReason::MaxTokens,
        StopReason::MaxTurnRequests => RuntimePromptStopReason::MaxTurnRequests,
        StopReason::Refusal => RuntimePromptStopReason::Refusal,
        StopReason::Cancelled => RuntimePromptStopReason::Cancelled,
        _ => RuntimePromptStopReason::Other,
    }
}

fn validate_cursor(cursor: Option<String>) -> Result<Option<String>, RuntimeError> {
    cursor
        .map(|cursor| validate_text(cursor, MAX_CURSOR_BYTES, "session cursor"))
        .transpose()
}

fn validate_optional_text(
    value: Option<String>,
    maximum: usize,
) -> Result<Option<String>, RuntimeError> {
    value
        .map(|value| validate_text(value, maximum, "runtime response text"))
        .transpose()
}

fn validate_text(
    value: String,
    maximum: usize,
    label: &'static str,
) -> Result<String, RuntimeError> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(malformed_response(format!("{label} was malformed")));
    }
    Ok(value)
}

fn require_capability(advertised: bool, capability: &'static str) -> Result<(), RuntimeError> {
    if advertised {
        Ok(())
    } else {
        Err(runtime_error(
            RuntimeErrorCode::CapabilityUnavailable,
            format!("runtime did not advertise {capability}"),
            false,
        ))
    }
}

fn require_advertised_mode(
    shared: &RuntimeShared,
    session_id: &str,
    mode_id: &str,
) -> Result<(), RuntimeError> {
    let controls = shared
        .session_controls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let advertised = controls
        .get(session_id)
        .and_then(|controls| controls.modes.as_ref())
        .is_some_and(|modes| modes.available_modes.iter().any(|mode| mode.id == mode_id));
    require_capability(advertised, "the requested session mode")
}

fn require_advertised_model(
    capabilities: &RuntimeCapabilities,
    model_id: &str,
    reasoning_effort: Option<&str>,
) -> Result<(), RuntimeError> {
    let model = capabilities
        .models
        .as_ref()
        .and_then(|models| {
            models
                .available_models
                .iter()
                .find(|model| model.model_id == model_id)
        })
        .ok_or_else(|| {
            runtime_error(
                RuntimeErrorCode::CapabilityUnavailable,
                "requested model was not advertised",
                false,
            )
        })?;
    if let Some(reasoning_effort) = reasoning_effort
        && !model
            .reasoning_efforts
            .iter()
            .any(|effort| effort.value == reasoning_effort || effort.id == reasoning_effort)
    {
        return Err(runtime_error(
            RuntimeErrorCode::CapabilityUnavailable,
            "requested reasoning effort was not advertised",
            false,
        ));
    }
    Ok(())
}

fn require_advertised_config(
    shared: &RuntimeShared,
    session_id: &str,
    config_id: &str,
    value: &RuntimeConfigValue,
) -> Result<(), RuntimeError> {
    let controls = shared
        .session_controls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let option = controls
        .get(session_id)
        .and_then(|controls| {
            controls
                .config_options
                .iter()
                .find(|option| option.id == config_id)
        })
        .ok_or_else(|| {
            runtime_error(
                RuntimeErrorCode::CapabilityUnavailable,
                "requested config option was not advertised",
                false,
            )
        })?;
    let valid = match (&option.kind, value) {
        (RuntimeConfigKind::Boolean { .. }, RuntimeConfigValue::Boolean(_)) => true,
        (RuntimeConfigKind::Select { options, .. }, RuntimeConfigValue::Select(value)) => {
            options.iter().any(|option| option.value == *value)
        }
        _ => false,
    };
    require_capability(valid, "the requested config value")
}

fn malformed_response(diagnostic: impl AsRef<str>) -> RuntimeError {
    runtime_error(RuntimeErrorCode::MalformedResponse, diagnostic, false)
}

fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .auth(AuthCapabilities::new().terminal(false))
        .elicitation(Some(
            ElicitationCapabilities::new().form(Some(ElicitationFormCapabilities::new())),
        ))
}

fn choose_auth_method(response: &InitializeResponse, requested: Option<&str>) -> Option<String> {
    if let Some(requested) = requested {
        return response
            .auth_methods
            .iter()
            .take(MAX_AUTH_METHODS)
            .find(|method| method.id().0.as_ref() == requested)
            .map(|method| method.id().to_string());
    }

    const PREFERRED: &[&str] = &["cached_token", "grok.com", "xai.api_key", "grok"];
    PREFERRED.iter().find_map(|preferred| {
        response
            .auth_methods
            .iter()
            .take(MAX_AUTH_METHODS)
            .find(|method| method.id().0.as_ref() == *preferred)
            .map(|method| method.id().to_string())
    })
}

fn normalize_capabilities(response: &InitializeResponse) -> RuntimeCapabilities {
    let mut auth_methods = BTreeSet::new();
    for method in response.auth_methods.iter().take(MAX_AUTH_METHODS) {
        auth_methods.insert(match method.id().0.as_ref() {
            "cached_token" => AuthenticationMethod::CachedToken,
            "grok.com" => AuthenticationMethod::GrokCom,
            "xai.api_key" => AuthenticationMethod::XaiApiKey,
            "grok" => AuthenticationMethod::Grok,
            _ => AuthenticationMethod::Other,
        });
    }

    let agent = response.agent_info.as_ref();
    let product = agent.map_or(RuntimeAgentProduct::Other, |agent| {
        if matches!(agent.name.as_str(), "grok" | "grok-build" | "Grok Build") {
            RuntimeAgentProduct::GrokBuild
        } else {
            RuntimeAgentProduct::Other
        }
    });
    let version = agent.and_then(|agent| safe_label(&agent.version));
    let session = &response.agent_capabilities.session_capabilities;
    let models = normalize_serializable_session_response(response).models;

    RuntimeCapabilities {
        protocol_version: 1,
        agent: RuntimeAgent { product, version },
        authentication_methods: auth_methods.into_iter().collect(),
        sessions: RuntimeSessionCapabilities {
            create: true,
            prompt: true,
            cancel: true,
            list: session.list.is_some(),
            load: response.agent_capabilities.load_session,
            resume: session.resume.is_some(),
            close: session.close.is_some(),
        },
        models,
    }
}

fn safe_label(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= MAX_SAFE_LABEL_BYTES
        && !value.chars().any(char::is_control))
    .then(|| value.to_owned())
}

fn runtime_error(
    code: RuntimeErrorCode,
    diagnostic: impl AsRef<str>,
    recoverable: bool,
) -> RuntimeError {
    RuntimeError {
        code,
        diagnostic: RedactedDiagnostic::new(diagnostic),
        recoverable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{AuthMethod, AuthMethodAgent};
    use serde_json::json;

    #[test]
    fn authentication_uses_only_bounded_advertised_methods() {
        let advertised = InitializeResponse::new(ProtocolVersion::V1).auth_methods(vec![
            auth_method("private-auth", "Private"),
            auth_method("grok.com", "Grok web"),
            auth_method("cached_token", "Cached token"),
        ]);

        assert_eq!(
            choose_auth_method(&advertised, None).as_deref(),
            Some("cached_token")
        );
        assert_eq!(
            choose_auth_method(&advertised, Some("grok.com")).as_deref(),
            Some("grok.com")
        );
        assert!(choose_auth_method(&advertised, Some("not-advertised")).is_none());

        let unknown_only = InitializeResponse::new(ProtocolVersion::V1)
            .auth_methods(vec![auth_method("private-auth", "Private")]);
        assert!(choose_auth_method(&unknown_only, None).is_none());
    }

    #[test]
    fn execute_permissions_without_an_absolute_working_directory_fail_closed() {
        let request: RequestPermissionRequest = serde_json::from_value(json!({
            "sessionId": "fixture-session",
            "toolCall": {
                "toolCallId": "fixture-tool",
                "title": "Ambiguous command",
                "kind": "execute",
                "status": "pending",
                "rawInput": {
                    "command": "fixture-tool --check",
                    "cwd": "relative-workspace"
                }
            },
            "options": [
                { "optionId": "allow", "name": "Allow", "kind": "allow_once" }
            ]
        }))
        .expect("fixture request should deserialize");

        assert!(
            normalize_permission_request(&request, "gui-interaction-1".to_owned()).is_none(),
            "an ambiguous command must be rejected before it reaches the GUI"
        );
    }

    #[test]
    fn only_the_elicitation_mode_the_gui_can_render_is_advertised() {
        let capabilities = serde_json::to_value(client_capabilities())
            .expect("client capabilities should serialize");

        assert!(
            capabilities.pointer("/elicitation/form").is_some(),
            "single-field forms should be advertised"
        );
        assert!(
            capabilities.pointer("/elicitation/url").is_none(),
            "URL elicitation must not be advertised without a safe GUI flow"
        );
    }

    #[test]
    fn elicitation_answers_must_match_the_advertised_field_and_choice() {
        let kind = ElicitationKind::Choice {
            field_id: "color".to_owned(),
            label: Some("Color".to_owned()),
            options: vec!["blue".to_owned(), "green".to_owned()],
            multiple: false,
        };
        let invalid_field = ElicitationDecision::Accept {
            content: BTreeMap::from([(
                "other".to_owned(),
                ElicitationValue::String("blue".to_owned()),
            )]),
        };
        let invalid_choice = ElicitationDecision::Accept {
            content: BTreeMap::from([(
                "color".to_owned(),
                ElicitationValue::String("red".to_owned()),
            )]),
        };
        let valid_choice = ElicitationDecision::Accept {
            content: BTreeMap::from([(
                "color".to_owned(),
                ElicitationValue::String("green".to_owned()),
            )]),
        };

        assert!(validate_elicitation_decision(&kind, &invalid_field).is_err());
        assert!(validate_elicitation_decision(&kind, &invalid_choice).is_err());
        assert!(validate_elicitation_decision(&kind, &valid_choice).is_ok());
    }

    fn auth_method(id: &str, name: &str) -> AuthMethod {
        AuthMethod::Agent(AuthMethodAgent::new(id.to_owned(), name.to_owned()))
    }

    #[test]
    fn missing_workspaces_are_rejected_before_an_acp_request_is_built() {
        let missing = std::env::temp_dir().join(format!(
            "grok-runtime-missing-workspace-{}",
            std::process::id()
        ));

        let error = canonicalize_workspace(missing).expect_err("workspace must already exist");
        assert_eq!(error.code, RuntimeErrorCode::InvalidWorkspace);
    }

    #[test]
    fn workspace_identity_does_not_treat_distinct_directories_as_the_same_session_home() {
        let first = std::env::temp_dir().join(format!(
            "grok-runtime-workspace-a-{}",
            std::process::id()
        ));
        let second = std::env::temp_dir().join(format!(
            "grok-runtime-workspace-b-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&first).expect("first workspace");
        std::fs::create_dir_all(&second).expect("second workspace");

        assert!(same_workspace(&first, &first));
        assert!(!same_workspace(&first, &second));

        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    #[tokio::test]
    async fn a_slow_event_consumer_gets_an_explicit_bounded_lag_error() {
        let runtime = GrokRuntime::from_target(RuntimeTarget {
            executable: PathBuf::from("unused-test-runtime"),
            arguments: Vec::new(),
            environment: Vec::new(),
            auth_method: None,
            request_timeout: Duration::from_secs(1),
        });
        let mut events = runtime.subscribe();

        for _ in 0..=EVENT_CHANNEL_CAPACITY {
            runtime.shared.emit(RuntimeEvent::RuntimeStateChanged {
                state: RuntimeState::Ready,
            });
        }

        assert!(matches!(
            events.recv().await,
            Err(broadcast::error::RecvError::Lagged(1))
        ));
    }
}
