use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthCapabilities, AuthenticateRequest, CancelNotification, ClientCapabilities,
    CloseSessionRequest, ContentBlock, CreateElicitationRequest, CreateElicitationResponse,
    ElicitationAction, ElicitationCapabilities, ElicitationFormCapabilities,
    ElicitationUrlCapabilities, InitializeRequest, InitializeResponse, ListSessionsRequest,
    LoadSessionRequest, NewSessionRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest,
    SelectedPermissionOutcome, SessionId, SessionNotification, SessionUpdate,
    SetSessionModeRequest, TextContent,
};
#[cfg(not(windows))]
use agent_client_protocol::{AcpAgent, AcpAgentConfig};
use agent_client_protocol::{Agent, ConnectionTo};
use serde::Serialize;
use serde_json::{Map, Value, json};
use thiserror::Error;

#[cfg(windows)]
use grok_runtime::WindowsAcpProcess;

mod control_probe;
#[cfg(windows)]
mod managed_restart;

use control_probe::{CurrentModelSelection, response_confirms_model};
pub use grok_runtime::{
    SanitizedWireFrame, WireCapture, WireDirection, WireFrameKind, WireSummary,
};
#[cfg(windows)]
pub use managed_restart::{ManagedRestartReport, run_managed_restart_probe};

pub const ACP_SDK_VERSION: &str = "2.0.0";
const MAX_AUTH_METHODS_TO_SCAN: usize = 64;

// Windows process startup and Job Object assignment are intentionally serialized.
// Starting several compatibility runtimes at once caused bounded probes to spend
// their entire work window competing in process setup before ACP initialize.
#[cfg(windows)]
static PROBE_EXECUTION_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Debug)]
pub struct ProbeTarget {
    executable: PathBuf,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
}

impl ProbeTarget {
    #[must_use]
    pub fn new(
        executable: impl Into<PathBuf>,
        arguments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            executable: executable.into(),
            arguments: arguments.into_iter().map(Into::into).collect(),
            environment: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    #[cfg(not(windows))]
    fn process_config(&self) -> AcpAgentConfig {
        AcpAgentConfig::new(self.executable.clone())
            .args(self.arguments.clone())
            .envs(self.environment.clone())
    }

    fn summary(&self) -> TargetSummary {
        const MAX_REPORTED_COUNT: usize = u16::MAX as usize;

        let product = self
            .executable
            .file_name()
            .and_then(|name| name.to_str())
            .map_or(TargetProduct::Other, |name| {
                if name.eq_ignore_ascii_case("grok") || name.eq_ignore_ascii_case("grok.exe") {
                    TargetProduct::GrokBuild
                } else {
                    TargetProduct::Other
                }
            });
        let launch_shape = if matches!(product, TargetProduct::GrokBuild)
            && (self
                .arguments
                .iter()
                .map(String::as_str)
                .eq(["agent", "stdio"])
                || self.arguments.iter().map(String::as_str).eq([
                    "--no-auto-update",
                    "agent",
                    "--no-leader",
                    "stdio",
                ])) {
            TargetLaunchShape::GrokAgentStdio
        } else {
            TargetLaunchShape::Other
        };

        TargetSummary {
            product,
            launch_shape,
            argument_count: self.arguments.len().min(MAX_REPORTED_COUNT) as u16,
            argument_count_capped: self.arguments.len() > MAX_REPORTED_COUNT,
            environment_override_count: self.environment.len().min(MAX_REPORTED_COUNT) as u16,
            environment_override_count_capped: self.environment.len() > MAX_REPORTED_COUNT,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ProbeKind {
    InitializeOnly,
    Lifecycle(LifecycleOptions),
    Controls(ControlOptions),
}

#[derive(Clone, Debug)]
pub struct LifecycleOptions {
    pub workspace: PathBuf,
    pub prompt: String,
    pub auth_method: Option<String>,
    pub exercise_cancel: bool,
}

#[derive(Clone, Debug)]
pub struct ControlOptions {
    pub workspace: PathBuf,
    pub auth_method: Option<String>,
}

#[derive(Clone, Debug)]
enum SessionExercise {
    Lifecycle {
        prompt: String,
        exercise_cancel: bool,
    },
    Controls,
}

#[derive(Clone, Debug)]
struct SessionProbeOptions {
    workspace: PathBuf,
    auth_method: Option<String>,
    exercise: SessionExercise,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub sdk_version: &'static str,
    pub wire_protocol: u16,
    pub target: TargetSummary,
    pub initialize: InitializeSummary,
    pub steps: Vec<StepObservation>,
    pub event_counts: BTreeMap<String, u64>,
    pub wire: WireSummary,
}

#[cfg(windows)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedInitializeReport {
    pub sdk_version: &'static str,
    pub wire_protocol: u16,
    pub target: TargetSummary,
    pub initialize: InitializeSummary,
    pub steps: Vec<StepObservation>,
    pub process: ManagedProcessSummary,
}

#[cfg(windows)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedProcessSummary {
    pub containment: &'static str,
    pub stderr_line_count: u64,
    pub stderr_byte_count: u64,
    pub stderr_truncated_line_count: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSummary {
    pub product: TargetProduct,
    pub launch_shape: TargetLaunchShape,
    pub argument_count: u16,
    pub argument_count_capped: bool,
    pub environment_override_count: u16,
    pub environment_override_count_capped: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetProduct {
    GrokBuild,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetLaunchShape {
    GrokAgentStdio,
    Other,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeSummary {
    pub protocol_version: Value,
    pub agent_info: Option<AgentInfoSummary>,
    pub agent_capabilities: Value,
    pub auth_methods: Vec<AuthMethodSummary>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfoSummary {
    pub product: AgentProduct,
    pub name_present: bool,
    pub title_present: bool,
    pub version_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProduct {
    GrokBuild,
    Other,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionCommandSummary {
    pub command_succeeded: bool,
    pub json_object: bool,
    pub current_version_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    pub channel: VersionChannel,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionChannel {
    Stable,
    Beta,
    Nightly,
    Dev,
    Canary,
    Other,
    Missing,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethodSummary {
    pub method: AuthMethodKind,
    pub name_present: bool,
    pub description_present: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethodKind {
    CachedToken,
    GrokCom,
    XaiApiKey,
    Grok,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepObservation {
    step: String,
    status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<Value>,
}

impl StepObservation {
    fn confirmed(step: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            status: StepStatus::Confirmed,
            detail: None,
        }
    }

    fn confirmed_with(step: impl Into<String>, detail: Value) -> Self {
        Self {
            step: step.into(),
            status: StepStatus::Confirmed,
            detail: Some(detail),
        }
    }

    fn skipped(step: impl Into<String>, reason: &'static str) -> Self {
        Self {
            step: step.into(),
            status: StepStatus::Skipped,
            detail: Some(json!({ "reason": reason })),
        }
    }

    fn failed(step: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            status: StepStatus::Failed,
            detail: Some(json!({ "reason": "request_failed" })),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Confirmed,
    Skipped,
    Failed,
}

#[derive(Clone, Debug, Error)]
pub enum ProbeError {
    #[error("ACP probe exceeded its {0:?} deadline")]
    Timeout(Duration),
    #[error("ACP protocol operation failed ({0})")]
    Protocol(ProtocolFailureCode),
    #[error("workspace is not an existing directory")]
    InvalidWorkspace,
    #[error("failed to canonicalize workspace")]
    CanonicalizeWorkspace,
    #[error("managed restart probe failed during {0}")]
    ManagedRestart(&'static str),
    #[error("ACP probe failed during {0}")]
    ProbeFailed(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolFailureCode {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    InternalError,
    RequestCancelled,
    AuthRequired,
    ResourceNotFound,
    Other,
}

impl std::fmt::Display for ProtocolFailureCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Self::ParseError => "parse_error",
            Self::InvalidRequest => "invalid_request",
            Self::MethodNotFound => "method_not_found",
            Self::InvalidParams => "invalid_params",
            Self::InternalError => "internal_error",
            Self::RequestCancelled => "request_cancelled",
            Self::AuthRequired => "auth_required",
            Self::ResourceNotFound => "resource_not_found",
            Self::Other => "other",
        };
        formatter.write_str(code)
    }
}

impl From<agent_client_protocol::Error> for ProbeError {
    fn from(error: agent_client_protocol::Error) -> Self {
        use agent_client_protocol::ErrorCode;

        let code = match error.code {
            ErrorCode::ParseError => ProtocolFailureCode::ParseError,
            ErrorCode::InvalidRequest => ProtocolFailureCode::InvalidRequest,
            ErrorCode::MethodNotFound => ProtocolFailureCode::MethodNotFound,
            ErrorCode::InvalidParams => ProtocolFailureCode::InvalidParams,
            ErrorCode::InternalError => ProtocolFailureCode::InternalError,
            ErrorCode::RequestCancelled => ProtocolFailureCode::RequestCancelled,
            ErrorCode::AuthRequired => ProtocolFailureCode::AuthRequired,
            ErrorCode::ResourceNotFound => ProtocolFailureCode::ResourceNotFound,
            _ => ProtocolFailureCode::Other,
        };
        Self::Protocol(code)
    }
}

#[derive(Debug, Error)]
#[error("{failure}")]
pub struct ProbeRunError {
    #[source]
    failure: ProbeError,
    partial_report: Option<ProbeReport>,
}

impl ProbeRunError {
    fn without_report(failure: ProbeError) -> Self {
        Self {
            failure,
            partial_report: None,
        }
    }

    fn with_report(failure: ProbeError, partial_report: Option<ProbeReport>) -> Self {
        Self {
            failure,
            partial_report,
        }
    }

    #[must_use]
    pub fn partial_report(&self) -> Option<&ProbeReport> {
        self.partial_report.as_ref()
    }

    #[must_use]
    pub fn failure(&self) -> &ProbeError {
        &self.failure
    }
}

#[derive(Clone)]
struct ProbeExecution {
    report: ProbeReport,
    failure: Option<ProbeError>,
}

#[derive(Default)]
struct EventCollector {
    counts: BTreeMap<String, u64>,
}

impl EventCollector {
    fn increment(&mut self, event: &'static str) {
        *self.counts.entry(event.to_owned()).or_default() += 1;
    }
}

pub async fn run_probe(
    target: ProbeTarget,
    kind: ProbeKind,
    deadline: Duration,
) -> Result<ProbeReport, ProbeRunError> {
    #[cfg(windows)]
    let _execution_guard = PROBE_EXECUTION_GATE.lock().await;

    let session_probe = match kind {
        ProbeKind::InitializeOnly => None,
        ProbeKind::Lifecycle(options) => Some(SessionProbeOptions {
            workspace: canonicalize_workspace(options.workspace)
                .map_err(ProbeRunError::without_report)?,
            auth_method: options.auth_method,
            exercise: SessionExercise::Lifecycle {
                prompt: options.prompt,
                exercise_cancel: options.exercise_cancel,
            },
        }),
        ProbeKind::Controls(options) => Some(SessionProbeOptions {
            workspace: canonicalize_workspace(options.workspace)
                .map_err(ProbeRunError::without_report)?,
            auth_method: options.auth_method,
            exercise: SessionExercise::Controls,
        }),
    };

    let started = tokio::time::Instant::now();
    let overall_expires = started + deadline;
    let cleanup_reserve = session_probe
        .as_ref()
        .map_or(Duration::ZERO, |_| probe_cleanup_reserve(deadline));
    let teardown_reserve = (cleanup_reserve / 4).min(Duration::from_millis(250));
    let work_expires = overall_expires
        .checked_sub(cleanup_reserve)
        .unwrap_or(started);
    let close_expires = overall_expires
        .checked_sub(teardown_reserve)
        .unwrap_or(started);

    let wire = WireCapture::default();
    let events = Arc::new(Mutex::new(EventCollector::default()));
    let completed_execution = Arc::new(Mutex::new(None::<ProbeExecution>));
    #[cfg(windows)]
    let agent = WindowsAcpProcess::new(target.executable.clone())
        .args(target.arguments.clone())
        .envs(target.environment.clone())
        .wire_capture(wire.clone());
    #[cfg(not(windows))]
    let agent = {
        let wire_for_debug = wire.clone();
        AcpAgent::new(target.process_config()).with_debug(move |line, direction| {
            wire_for_debug.record_transport_line(line, direction);
        })
    };

    let events_for_notification = Arc::clone(&events);
    let events_for_permission = Arc::clone(&events);
    let events_for_elicitation = Arc::clone(&events);
    let completed_for_connection = Arc::clone(&completed_execution);
    let target_summary = target.summary();

    let connection = agent_client_protocol::Client
        .builder()
        .name("grok-build-gui-phase-0")
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                let kind = session_update_name(&notification.update);
                lock_unpoisoned(&events_for_notification).increment(kind);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |
                request: RequestPermissionRequest,
                responder,
                _connection,
            | {
                {
                    let mut events = lock_unpoisoned(&events_for_permission);
                    events.increment("permission_requested");
                }
                let rejected = request
                    .options
                    .iter()
                    .find(|option| {
                        matches!(
                            option.kind,
                            agent_client_protocol::schema::v1::PermissionOptionKind::RejectOnce
                                | agent_client_protocol::schema::v1::PermissionOptionKind::RejectAlways
                        )
                    })
                    .map(|option| option.option_id.clone());

                let outcome = rejected.map_or(
                    RequestPermissionOutcome::Cancelled,
                    |option_id| {
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
                    },
                );
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |
                _request: CreateElicitationRequest,
                responder,
                _connection,
            | {
                {
                    let mut events = lock_unpoisoned(&events_for_elicitation);
                    events.increment("elicitation_requested");
                }
                responder.respond(CreateElicitationResponse::new(ElicitationAction::Cancel))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| async move {
            let result: Result<ProbeExecution, ProbeError> = async {
            let capabilities = probe_client_capabilities();
            let initialize = tokio::time::timeout_at(
                work_expires,
                connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(capabilities),
                    )
                    .block_task(),
            )
                .await
                .map_err(|_| ProbeError::ProbeFailed("initialize_request_timed_out"))?
                .map_err(|_| ProbeError::ProbeFailed("initialize_request_failed"))?;
            let current_model = CurrentModelSelection::from_initialize(&initialize);
            let initialize_summary = summarize_initialize(&initialize);
            let mut steps = vec![StepObservation::confirmed("initialize")];

            if let Some(options) = session_probe {
                let auth_method = choose_auth_method(&initialize, options.auth_method.as_deref());
                if options.auth_method.is_some() && auth_method.is_none() {
                    steps.push(StepObservation::failed("authenticate"));
                    return Ok(finish_probe_execution(
                        &target_summary,
                        &initialize_summary,
                        steps,
                        &events,
                        &wire,
                        Some(ProbeError::ProbeFailed(
                            "requested_auth_method_not_advertised",
                        )),
                    ));
                }
                if let Some(method_id) = auth_method {
                    let mut meta = Map::new();
                    meta.insert("headless".to_owned(), Value::Bool(true));
                    let auth_result = tokio::time::timeout_at(
                        work_expires,
                        connection
                            .send_request(AuthenticateRequest::new(method_id.clone()).meta(meta))
                            .block_task(),
                    )
                    .await;
                    let auth_failure = match auth_result {
                        Ok(Ok(_)) => None,
                        Ok(Err(_)) => Some(ProbeError::ProbeFailed(
                            "authenticate_request_failed",
                        )),
                        Err(_) => Some(ProbeError::ProbeFailed(
                            "authenticate_request_timed_out",
                        )),
                    };
                    if let Some(failure) = auth_failure {
                        steps.push(StepObservation::failed("authenticate"));
                        return Ok(finish_probe_execution(
                            &target_summary,
                            &initialize_summary,
                            steps,
                            &events,
                            &wire,
                            Some(failure),
                        ));
                    }
                    steps.push(authenticated_step(&method_id));
                } else {
                    let reason = if initialize.auth_methods.is_empty() {
                        "agent_advertised_no_auth_methods"
                    } else {
                        "agent_advertised_no_supported_auth_methods"
                    };
                    steps.push(StepObservation::skipped(
                        "authenticate",
                        reason,
                    ));
                }

                if initialize
                    .agent_capabilities
                    .session_capabilities
                    .close
                    .is_none()
                {
                    steps.push(StepObservation::failed("session/close_capability"));
                    return Ok(finish_probe_execution(
                        &target_summary,
                        &initialize_summary,
                        steps,
                        &events,
                        &wire,
                        Some(ProbeError::ProbeFailed("session_close_not_advertised")),
                    ));
                }

                let mut new_session_request = NewSessionRequest::new(options.workspace.clone());
                if matches!(&options.exercise, SessionExercise::Controls)
                    && let Some(selection) = &current_model
                {
                    let mut meta = Map::new();
                    meta.insert(
                        "modelId".to_owned(),
                        Value::String(selection.model_id.clone()),
                    );
                    if let Some(reasoning_effort) = &selection.reasoning_effort {
                        meta.insert(
                            "reasoningEffort".to_owned(),
                            Value::String(reasoning_effort.clone()),
                        );
                    }
                    new_session_request = new_session_request.meta(meta);
                }
                let new_session_result = tokio::time::timeout_at(
                    work_expires,
                    connection.send_request(new_session_request).block_task(),
                )
                .await;
                let new_session = match new_session_result {
                    Ok(Ok(response)) => response,
                    Ok(Err(_)) => {
                        steps.push(StepObservation::failed("session/new"));
                        return Ok(finish_probe_execution(
                            &target_summary,
                            &initialize_summary,
                            steps,
                            &events,
                            &wire,
                            Some(ProbeError::ProbeFailed("session_new_request_failed")),
                        ));
                    }
                    Err(_) => {
                        steps.push(StepObservation::failed("session/new"));
                        return Ok(finish_probe_execution(
                            &target_summary,
                            &initialize_summary,
                            steps,
                            &events,
                            &wire,
                            Some(ProbeError::ProbeFailed("session_new_request_timed_out")),
                        ));
                    }
                };
                let config_option_count =
                    new_session.config_options.as_ref().map_or(0, Vec::len);
                let session_id = new_session.session_id;
                steps.push(StepObservation::confirmed_with(
                    "session/new",
                    json!({
                        "modesPresent": new_session.modes.is_some(),
                        "configOptionCount": config_option_count,
                    }),
                ));

                let exercise_failure = match tokio::time::timeout_at(work_expires, async {
                    let mut exercise_failure = None;
                    match options.exercise {
                    SessionExercise::Lifecycle {
                        prompt,
                        exercise_cancel,
                    } => {
                        let prompt_response = connection
                            .send_request(PromptRequest::new(
                                session_id.clone(),
                                vec![ContentBlock::Text(TextContent::new(prompt))],
                            ))
                            .block_task()
                            .await;
                        match prompt_response {
                            Ok(prompt_response) => {
                                steps.push(StepObservation::confirmed_with(
                                    "session/prompt",
                                    json!({ "stopReason": prompt_response.stop_reason }),
                                ));

                                if exercise_cancel {
                                    let pending = connection.send_request(PromptRequest::new(
                                        session_id.clone(),
                                        vec![ContentBlock::Text(TextContent::new(
                                            "Wait until cancelled. Do not use tools or modify files.",
                                        ))],
                                    ));
                                    tokio::time::sleep(Duration::from_millis(150)).await;
                                    match connection.send_notification(CancelNotification::new(
                                        session_id.clone(),
                                    )) {
                                        Ok(()) => match pending.block_task().await {
                                            Ok(cancelled) => {
                                                steps.push(StepObservation::confirmed_with(
                                                    "session/cancel",
                                                    json!({ "stopReason": cancelled.stop_reason }),
                                                ));
                                            }
                                            Err(_) => {
                                                steps.push(StepObservation::failed(
                                                    "session/cancel",
                                                ));
                                                exercise_failure = Some(
                                                    "session_cancelled_prompt_request_failed",
                                                );
                                            }
                                        },
                                        Err(_) => {
                                            steps.push(StepObservation::failed("session/cancel"));
                                            exercise_failure =
                                                Some("session_cancel_notification_failed");
                                        }
                                    }
                                } else {
                                    steps.push(StepObservation::skipped(
                                        "session/cancel",
                                        "not_requested",
                                    ));
                                }

                                if exercise_failure.is_none() {
                                    exercise_failure = collect_session_recovery_steps(
                                        &connection,
                                        &initialize,
                                        &options.workspace,
                                        &session_id,
                                        &mut steps,
                                    )
                                    .await;
                                }
                            }
                            Err(_) => {
                                steps.push(StepObservation::failed("session/prompt"));
                                exercise_failure = Some("session_prompt_request_failed");
                            }
                        }
                    }
                    SessionExercise::Controls => {
                        if let Some(selection) = current_model {
                            let expected_model = selection.model_id.clone();
                            let reasoning_effort_present = selection.reasoning_effort.is_some();
                            match connection
                                .send_request(selection.into_request(session_id.to_string()))
                                .block_task()
                                .await
                            {
                                Ok(response)
                                    if response_confirms_model(&response, &expected_model) =>
                                {
                                    steps.push(StepObservation::confirmed_with(
                                        "session/set_model",
                                        json!({
                                            "rpcAccepted": true,
                                            "sameAdvertisedModel": true,
                                            "reasoningEffortPresent": reasoning_effort_present,
                                            "responseConfirmedModel": true,
                                        }),
                                    ));
                                }
                                Ok(_) => {
                                    steps.push(StepObservation::failed("session/set_model"));
                                    exercise_failure =
                                        Some("session_set_model_response_mismatch");
                                }
                                Err(_) => {
                                    steps.push(StepObservation::failed("session/set_model"));
                                    exercise_failure = Some("session_set_model_request_failed");
                                }
                            }
                        } else {
                            steps.push(StepObservation::skipped(
                                "session/set_model",
                                "current_model_not_advertised",
                            ));
                        }

                        match connection
                            .send_request(SetSessionModeRequest::new(session_id.clone(), "plan"))
                            .block_task()
                            .await
                        {
                            Ok(_) => {
                                steps.push(StepObservation::confirmed("session/set_mode:plan"));
                            }
                            Err(_) => {
                                steps.push(StepObservation::failed("session/set_mode:plan"));
                                exercise_failure
                                    .get_or_insert("session_set_mode_plan_request_failed");
                            }
                        }
                        match connection
                            .send_request(SetSessionModeRequest::new(session_id.clone(), "default"))
                            .block_task()
                            .await
                        {
                            Ok(_) => {
                                steps.push(StepObservation::confirmed("session/set_mode:default"));
                            }
                            Err(_) => {
                                steps.push(StepObservation::failed("session/set_mode:default"));
                                exercise_failure
                                    .get_or_insert("session_set_mode_default_request_failed");
                            }
                        }

                        let reason = if config_option_count == 0 {
                            "agent_advertised_no_standard_config_options"
                        } else {
                            "legacy_control_probe_does_not_mutate_standard_config_options"
                        };
                        steps.push(StepObservation::skipped(
                            "session/set_config_option",
                            reason,
                        ));
                        steps.push(StepObservation::skipped(
                            "_x.ai/yolo_mode_changed",
                            "global_permission_state_is_not_safe_to_probe",
                        ));
                    }
                    }
                    exercise_failure
                })
                .await
                {
                    Ok(failure) => failure,
                    Err(_) => {
                        steps.push(StepObservation::failed("session/exercise"));
                        Some("session_exercise_timed_out")
                    }
                };

                let mut final_failure = exercise_failure.map(ProbeError::ProbeFailed);
                match tokio::time::timeout_at(
                    close_expires,
                    connection
                        .send_request(CloseSessionRequest::new(session_id))
                        .block_task(),
                )
                .await
                {
                    Ok(Ok(_)) => steps.push(StepObservation::confirmed("session/close")),
                    Ok(Err(_)) => {
                        steps.push(StepObservation::failed("session/close"));
                        final_failure = Some(ProbeError::ProbeFailed(
                            "session_close_request_failed",
                        ));
                    }
                    Err(_) => {
                        steps.push(StepObservation::failed("session/close"));
                        final_failure = Some(ProbeError::ProbeFailed(
                            "session_close_request_timed_out",
                        ));
                    }
                }

                return Ok(finish_probe_execution(
                    &target_summary,
                    &initialize_summary,
                    steps,
                    &events,
                    &wire,
                    final_failure,
                ));
            }

            Ok(finish_probe_execution(
                &target_summary,
                &initialize_summary,
                steps,
                &events,
                &wire,
                None,
            ))
            }
            .await;
            if let Ok(execution) = &result {
                *lock_unpoisoned(&completed_for_connection) = Some(execution.clone());
            }
            Ok::<_, agent_client_protocol::Error>(result)
        });

    let execution = match tokio::time::timeout_at(overall_expires, connection).await {
        Err(_) => {
            return Err(failure_with_completed_report(
                ProbeError::Timeout(deadline),
                &completed_execution,
            ));
        }
        Ok(Err(_)) => {
            return Err(failure_with_completed_report(
                ProbeError::ProbeFailed("connection_failed"),
                &completed_execution,
            ));
        }
        Ok(Ok(Err(error))) => {
            return Err(failure_with_completed_report(error, &completed_execution));
        }
        Ok(Ok(Ok(execution))) => execution,
    };

    match execution.failure {
        Some(failure) => Err(ProbeRunError::with_report(failure, Some(execution.report))),
        None => Ok(execution.report),
    }
}

fn probe_cleanup_reserve(deadline: Duration) -> Duration {
    (deadline / 4).min(Duration::from_secs(5))
}

fn failure_with_completed_report(
    fallback: ProbeError,
    completed_execution: &Arc<Mutex<Option<ProbeExecution>>>,
) -> ProbeRunError {
    let Some(execution) = lock_unpoisoned(completed_execution).take() else {
        return ProbeRunError::without_report(fallback);
    };
    ProbeRunError::with_report(
        execution.failure.unwrap_or(fallback),
        Some(execution.report),
    )
}

fn finish_probe_execution(
    target: &TargetSummary,
    initialize: &InitializeSummary,
    steps: Vec<StepObservation>,
    events: &Arc<Mutex<EventCollector>>,
    wire: &WireCapture,
    failure: Option<ProbeError>,
) -> ProbeExecution {
    ProbeExecution {
        report: ProbeReport {
            sdk_version: ACP_SDK_VERSION,
            wire_protocol: 1,
            target: target.clone(),
            initialize: initialize.clone(),
            steps,
            event_counts: lock_unpoisoned(events).counts.clone(),
            wire: wire.summary(),
        },
        failure,
    }
}

/// Initialize through the race-free Windows Job Object process boundary.
///
/// This intentionally stops before authentication or session creation. The
/// returned report includes only negotiated capability summaries and stderr
/// counters; retained diagnostic text stays inside the runtime boundary.
#[cfg(windows)]
pub async fn run_managed_initialize_probe(
    target: ProbeTarget,
    deadline: Duration,
) -> Result<ManagedInitializeReport, ProbeError> {
    let target_summary = target.summary();
    let process = WindowsAcpProcess::new(target.executable)
        .args(target.arguments)
        .envs(target.environment);
    let diagnostics = process.diagnostics();
    let connection = agent_client_protocol::Client
        .builder()
        .name("grok-build-gui-phase-0-managed")
        .connect_with(process, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_capabilities(probe_client_capabilities()),
                )
                .block_task()
                .await
        });
    let initialize = tokio::time::timeout(deadline, connection)
        .await
        .map_err(|_| ProbeError::Timeout(deadline))?
        .map_err(ProbeError::from)?;
    let diagnostics = diagnostics.snapshot();

    Ok(ManagedInitializeReport {
        sdk_version: ACP_SDK_VERSION,
        wire_protocol: 1,
        target: target_summary,
        initialize: summarize_initialize(&initialize),
        steps: vec![
            StepObservation::confirmed("windows/job_object_containment"),
            StepObservation::confirmed("initialize"),
            StepObservation::skipped("authenticate", "initialize_only"),
            StepObservation::skipped("session/new", "initialize_only"),
        ],
        process: ManagedProcessSummary {
            containment: "suspended_then_job_assigned",
            stderr_line_count: diagnostics.total_lines,
            stderr_byte_count: diagnostics.total_bytes,
            stderr_truncated_line_count: diagnostics.truncated_lines,
        },
    })
}

fn probe_client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .auth(AuthCapabilities::new().terminal(false))
        .elicitation(Some(
            ElicitationCapabilities::new()
                .form(Some(ElicitationFormCapabilities::new()))
                .url(Some(ElicitationUrlCapabilities::new())),
        ))
}

fn canonicalize_workspace(workspace: PathBuf) -> Result<PathBuf, ProbeError> {
    if !workspace.is_dir() {
        return Err(ProbeError::InvalidWorkspace);
    }
    std::fs::canonicalize(&workspace).map_err(|_| ProbeError::CanonicalizeWorkspace)
}

async fn collect_session_recovery_steps(
    connection: &ConnectionTo<Agent>,
    initialize: &InitializeResponse,
    workspace: &std::path::Path,
    session_id: &SessionId,
    steps: &mut Vec<StepObservation>,
) -> Option<&'static str> {
    let mut failure = None;
    let session_capabilities = &initialize.agent_capabilities.session_capabilities;
    if session_capabilities.list.is_some() {
        match connection
            .send_request(ListSessionsRequest::new().cwd(Some(workspace.to_path_buf())))
            .block_task()
            .await
        {
            Ok(response) => steps.push(StepObservation::confirmed_with(
                "session/list",
                json!({
                    "sessionCount": response.sessions.len(),
                    "hasNextCursor": response.next_cursor.is_some(),
                }),
            )),
            Err(_) => {
                steps.push(StepObservation::failed("session/list"));
                failure = Some("session_list_request_failed");
            }
        }
    } else {
        steps.push(StepObservation::skipped(
            "session/list",
            "capability_not_advertised",
        ));
    }

    if initialize.agent_capabilities.load_session {
        match connection
            .send_request(LoadSessionRequest::new(
                session_id.clone(),
                workspace.to_path_buf(),
            ))
            .block_task()
            .await
        {
            Ok(_) => steps.push(StepObservation::confirmed("session/load")),
            Err(_) => {
                steps.push(StepObservation::failed("session/load"));
                failure.get_or_insert("session_load_request_failed");
            }
        }
    } else {
        steps.push(StepObservation::skipped(
            "session/load",
            "capability_not_advertised",
        ));
    }

    if session_capabilities.resume.is_some() {
        match connection
            .send_request(ResumeSessionRequest::new(
                session_id.clone(),
                workspace.to_path_buf(),
            ))
            .block_task()
            .await
        {
            Ok(_) => steps.push(StepObservation::confirmed("session/resume")),
            Err(_) => {
                steps.push(StepObservation::failed("session/resume"));
                failure.get_or_insert("session_resume_request_failed");
            }
        }
    } else {
        steps.push(StepObservation::skipped(
            "session/resume",
            "capability_not_advertised",
        ));
    }
    failure
}

fn choose_auth_method(initialize: &InitializeResponse, requested: Option<&str>) -> Option<String> {
    if let Some(requested) = requested {
        return initialize
            .auth_methods
            .iter()
            .take(MAX_AUTH_METHODS_TO_SCAN)
            .find(|method| method.id().0.as_ref() == requested)
            .map(|method| method.id().to_string());
    }

    const PREFERRED: &[&str] = &["cached_token", "grok.com", "xai.api_key", "grok"];
    PREFERRED
        .iter()
        .find_map(|preferred| {
            initialize
                .auth_methods
                .iter()
                .take(MAX_AUTH_METHODS_TO_SCAN)
                .find(|method| method.id().0.as_ref() == *preferred)
        })
        .map(|method| method.id().to_string())
}

fn authenticated_step(method_id: &str) -> StepObservation {
    StepObservation::confirmed_with(
        "authenticate",
        json!({ "method": auth_method_kind(method_id) }),
    )
}

fn summarize_initialize(response: &InitializeResponse) -> InitializeSummary {
    const REPORT_ORDER: [AuthMethodKind; 5] = [
        AuthMethodKind::CachedToken,
        AuthMethodKind::GrokCom,
        AuthMethodKind::XaiApiKey,
        AuthMethodKind::Grok,
        AuthMethodKind::Unknown,
    ];

    let agent_info = response.agent_info.as_ref().map(summarize_agent_info);
    let mut agent_capabilities =
        to_safe_value(&response.agent_capabilities).unwrap_or_else(|| Value::Object(Map::new()));
    strip_metadata(&mut agent_capabilities);

    let mut auth_method_presence = BTreeMap::new();
    for method in response.auth_methods.iter().take(MAX_AUTH_METHODS_TO_SCAN) {
        let kind = auth_method_kind(method.id().0.as_ref());
        let entry = auth_method_presence.entry(kind).or_insert((false, false));
        entry.0 |= !method.name().is_empty();
        entry.1 |= method.description().is_some();
    }
    let auth_methods = REPORT_ORDER
        .into_iter()
        .filter_map(|method| {
            auth_method_presence
                .get(&method)
                .map(|&(name_present, description_present)| AuthMethodSummary {
                    method,
                    name_present,
                    description_present,
                })
        })
        .collect();

    InitializeSummary {
        protocol_version: to_safe_value(&response.protocol_version).unwrap_or(Value::Null),
        agent_info,
        agent_capabilities,
        auth_methods,
    }
}

fn summarize_agent_info<T: Serialize>(agent_info: &T) -> AgentInfoSummary {
    let value = to_safe_value(agent_info).unwrap_or(Value::Null);
    let object = value.as_object();
    let name = object
        .and_then(|object| object.get("name"))
        .and_then(Value::as_str);
    let title = object
        .and_then(|object| object.get("title"))
        .and_then(Value::as_str);
    let version = object
        .and_then(|object| object.get("version"))
        .and_then(Value::as_str);

    AgentInfoSummary {
        product: match name {
            Some("grok" | "grok-build" | "Grok Build") => AgentProduct::GrokBuild,
            _ => AgentProduct::Other,
        },
        name_present: name.is_some_and(|name| !name.is_empty()),
        title_present: title.is_some_and(|title| !title.is_empty()),
        version_present: version.is_some(),
        version: version.and_then(safe_version_string),
    }
}

#[must_use]
pub fn summarize_version_command(stdout: &[u8], command_succeeded: bool) -> VersionCommandSummary {
    let value = serde_json::from_slice::<Value>(stdout).ok();
    let object = value.as_ref().and_then(Value::as_object);
    let current_version = object
        .and_then(|object| object.get("currentVersion"))
        .and_then(Value::as_str);
    let channel = object
        .and_then(|object| object.get("channel"))
        .and_then(Value::as_str);

    VersionCommandSummary {
        command_succeeded,
        json_object: object.is_some(),
        current_version_present: current_version.is_some(),
        current_version: current_version.and_then(safe_version_string),
        channel: match channel {
            Some("stable") => VersionChannel::Stable,
            Some("beta") => VersionChannel::Beta,
            Some("nightly") => VersionChannel::Nightly,
            Some("dev") => VersionChannel::Dev,
            Some("canary") => VersionChannel::Canary,
            Some(_) => VersionChannel::Other,
            None => VersionChannel::Missing,
        },
    }
}

fn safe_version_string(raw: &str) -> Option<String> {
    const MAX_VERSION_BYTES: usize = 96;
    const MIN_COMMIT_BYTES: usize = 7;
    const MAX_COMMIT_BYTES: usize = 64;

    if raw.is_empty() || raw.len() > MAX_VERSION_BYTES || !raw.is_ascii() {
        return None;
    }

    let (release, commit) = match raw.split_once(' ') {
        Some((release, commit)) => (release, Some(commit)),
        None => (raw, None),
    };
    if !is_reviewed_semver_release(release) {
        return None;
    }

    if let Some(commit) = commit {
        let commit = commit
            .strip_prefix('(')
            .and_then(|commit| commit.strip_suffix(')'))?;
        if !(MIN_COMMIT_BYTES..=MAX_COMMIT_BYTES).contains(&commit.len())
            || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
    }

    Some(raw.to_owned())
}

fn is_reviewed_semver_release(release: &str) -> bool {
    let (core, prerelease) = release
        .split_once('-')
        .map_or((release, None), |(core, prerelease)| {
            (core, Some(prerelease))
        });
    let mut components = core.split('.');
    if !(0..3).all(|_| components.next().is_some_and(is_semver_number))
        || components.next().is_some()
    {
        return false;
    }

    prerelease.is_none_or(|prerelease| {
        let (channel, sequence) = prerelease
            .split_once('.')
            .map_or((prerelease, None), |(channel, sequence)| {
                (channel, Some(sequence))
            });
        matches!(channel, "beta" | "nightly" | "dev" | "canary")
            && sequence.is_none_or(is_semver_number)
    })
}

fn is_semver_number(component: &str) -> bool {
    const MAX_COMPONENT_DIGITS: usize = 10;

    !component.is_empty()
        && component.len() <= MAX_COMPONENT_DIGITS
        && component.bytes().all(|byte| byte.is_ascii_digit())
        && (component == "0" || !component.starts_with('0'))
}

fn auth_method_kind(raw: &str) -> AuthMethodKind {
    match raw {
        "cached_token" => AuthMethodKind::CachedToken,
        "grok.com" => AuthMethodKind::GrokCom,
        "xai.api_key" => AuthMethodKind::XaiApiKey,
        "grok" => AuthMethodKind::Grok,
        _ => AuthMethodKind::Unknown,
    }
}

fn to_safe_value<T: Serialize>(value: &T) -> Option<Value> {
    serde_json::to_value(value).ok()
}

fn strip_metadata(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("_meta");
            for child in map.values_mut() {
                strip_metadata(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                strip_metadata(child);
            }
        }
        _ => {}
    }
}

fn session_update_name(update: &SessionUpdate) -> &'static str {
    match update {
        SessionUpdate::UserMessageChunk(_) => "user_message_chunk",
        SessionUpdate::AgentMessageChunk(_) => "agent_message_chunk",
        SessionUpdate::AgentThoughtChunk(_) => "thought_chunk",
        SessionUpdate::ToolCall(_) => "tool_call",
        SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
        SessionUpdate::Plan(_) => "plan_changed",
        SessionUpdate::AvailableCommandsUpdate(_) => "available_commands_changed",
        SessionUpdate::CurrentModeUpdate(_) => "mode_changed",
        SessionUpdate::ConfigOptionUpdate(_) => "config_options_changed",
        SessionUpdate::SessionInfoUpdate(_) => "session_info_changed",
        SessionUpdate::UsageUpdate(_) => "usage_changed",
        _ => "unknown_session_update",
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{AuthMethod, AuthMethodAgent};

    #[test]
    fn strips_nested_metadata_without_losing_capability_shape() {
        let mut value = json!({
            "loadSession": true,
            "_meta": { "secret": "do-not-keep" },
            "nested": [{ "_meta": { "path": "private" }, "enabled": true }],
        });
        strip_metadata(&mut value);
        assert_eq!(
            value,
            json!({ "loadSession": true, "nested": [{ "enabled": true }] })
        );
    }

    #[test]
    fn target_summary_never_retains_launch_strings() {
        let target = ProbeTarget::new(
            "C:/Users/ExampleUser/private-agent.exe",
            ["secret-command", "--token=xai-secret-value"],
        )
        .env("PRIVATE_TOKEN_NAME", "private-token-value");
        let summary = target.summary();
        let serialized = serde_json::to_string(&summary).expect("target summary should serialize");

        assert_eq!(summary.product, TargetProduct::Other);
        assert_eq!(summary.launch_shape, TargetLaunchShape::Other);
        assert_eq!(summary.argument_count, 2);
        assert_eq!(summary.environment_override_count, 1);
        for private_value in [
            "ExampleUser",
            "private-agent",
            "secret-command",
            "xai-secret-value",
            "PRIVATE_TOKEN_NAME",
            "private-token-value",
        ] {
            assert!(!serialized.contains(private_value));
        }

        let recognized = ProbeTarget::new("C:/tools/grok.exe", ["agent", "stdio"]).summary();
        assert_eq!(recognized.product, TargetProduct::GrokBuild);
        assert_eq!(recognized.launch_shape, TargetLaunchShape::GrokAgentStdio);
        let production = ProbeTarget::new(
            "C:/tools/grok.exe",
            ["--no-auto-update", "agent", "--no-leader", "stdio"],
        )
        .summary();
        assert_eq!(production.launch_shape, TargetLaunchShape::GrokAgentStdio);
        let wrong_order = ProbeTarget::new(
            "C:/tools/grok.exe",
            ["agent", "--no-auto-update", "--no-leader", "stdio"],
        )
        .summary();
        assert_eq!(wrong_order.launch_shape, TargetLaunchShape::Other);
    }

    #[test]
    fn auth_report_uses_bounded_deduplicated_semantic_methods() {
        let mut methods = vec![auth_method(
            "private/auth/C:/Users/ExampleUser/xai-secret-value",
            "secret method name",
        )];
        methods.extend((0..40).map(|index| {
            auth_method(
                &format!("unknown-private-auth-{index}"),
                "another secret name",
            )
        }));
        methods.push(auth_method("cached_token", "Cached token"));
        methods.push(auth_method("cached_token", "Duplicate cached token"));
        methods.push(auth_method("grok.com", "Grok web"));
        let response = InitializeResponse::new(ProtocolVersion::V1).auth_methods(methods);

        let summary = summarize_initialize(&response);
        let serialized =
            serde_json::to_string(&summary).expect("initialize summary should serialize");
        assert_eq!(summary.auth_methods.len(), 3);
        assert_eq!(summary.auth_methods[0].method, AuthMethodKind::CachedToken);
        assert_eq!(summary.auth_methods[1].method, AuthMethodKind::GrokCom);
        assert_eq!(summary.auth_methods[2].method, AuthMethodKind::Unknown);
        for private_value in [
            "ExampleUser",
            "xai-secret-value",
            "unknown-private-auth",
            "secret method name",
            "another secret name",
        ] {
            assert!(!serialized.contains(private_value));
        }
    }

    #[test]
    fn auth_selection_never_implicitly_chooses_unknown_method() {
        let private_id = "private-auth-C:/Users/ExampleUser/xai-secret-value";
        let response = InitializeResponse::new(ProtocolVersion::V1)
            .auth_methods(vec![auth_method(private_id, "Private")]);

        assert!(choose_auth_method(&response, None).is_none());
        assert_eq!(
            choose_auth_method(&response, Some(private_id)).as_deref(),
            Some(private_id)
        );
        assert!(choose_auth_method(&response, Some("not-advertised")).is_none());

        let serialized = serde_json::to_string(&authenticated_step(private_id))
            .expect("authentication step should serialize");
        assert!(serialized.contains("\"method\":\"unknown\""));
        for private_value in ["ExampleUser", "xai-secret-value", "private-auth"] {
            assert!(!serialized.contains(private_value));
        }

        let mut boundary_methods = (0..MAX_AUTH_METHODS_TO_SCAN)
            .map(|index| auth_method(&format!("unknown-{index}"), "Unknown"))
            .collect::<Vec<_>>();
        boundary_methods.push(auth_method("cached_token", "Out of bounds known method"));
        boundary_methods.push(auth_method(private_id, "Out of bounds requested method"));
        let boundary = InitializeResponse::new(ProtocolVersion::V1).auth_methods(boundary_methods);
        assert!(choose_auth_method(&boundary, None).is_none());
        assert!(choose_auth_method(&boundary, Some("cached_token")).is_none());
        assert!(choose_auth_method(&boundary, Some(private_id)).is_none());
        assert_eq!(summarize_initialize(&boundary).auth_methods.len(), 1);
        assert_eq!(
            summarize_initialize(&boundary).auth_methods[0].method,
            AuthMethodKind::Unknown
        );
    }

    #[test]
    fn agent_info_summary_projects_free_form_identity_to_fixed_safe_fields() {
        let summary = summarize_agent_info(&json!({
            "name": "private-agent-name-C:/Users/ExampleUser",
            "title": "secret title from a private workspace",
            "version": "C:/Users/ExampleUser/private-build",
            "_meta": { "token": "private-token-value" }
        }));
        let serialized = serde_json::to_string(&summary).expect("summary should serialize");

        assert!(matches!(summary.product, AgentProduct::Other));
        assert!(summary.name_present);
        assert!(summary.title_present);
        assert!(summary.version_present);
        assert!(summary.version.is_none());
        for private_value in [
            "private-agent-name",
            "secret title",
            "ExampleUser",
            "private-token-value",
        ] {
            assert!(!serialized.contains(private_value));
        }
    }

    #[test]
    fn version_command_summary_allowlists_version_grammar_and_known_channels() {
        let valid = summarize_version_command(
            br#"{"currentVersion":"1.0.5 (5115b46bc9)","channel":"stable"}"#,
            true,
        );
        assert_eq!(valid.current_version.as_deref(), Some("1.0.5 (5115b46bc9)"));
        assert!(matches!(valid.channel, VersionChannel::Stable));

        let adversarial = summarize_version_command(
            br#"{"currentVersion":"C:/Users/ExampleUser/private","channel":"private-channel","secret":"do-not-retain"}"#,
            true,
        );
        let serialized =
            serde_json::to_string(&adversarial).expect("version summary should serialize");
        assert!(adversarial.current_version_present);
        assert!(adversarial.current_version.is_none());
        assert!(matches!(adversarial.channel, VersionChannel::Other));
        for private_value in ["ExampleUser", "private-channel", "do-not-retain"] {
            assert!(!serialized.contains(private_value));
        }

        for unsafe_version in [
            "1-private-secret",
            "1.0.5-private-secret",
            "1.0.5+private-secret",
            "01.0.5",
            "1.0",
            "1.0.5-beta.private",
        ] {
            assert_eq!(
                safe_version_string(unsafe_version),
                None,
                "{unsafe_version}"
            );
        }
        for safe_version in ["1.0.5", "1.0.5-beta", "1.0.5-beta.2", "1.0.5 (5115b46bc9)"] {
            assert_eq!(
                safe_version_string(safe_version).as_deref(),
                Some(safe_version),
                "{safe_version}"
            );
        }
    }

    #[test]
    fn protocol_and_workspace_errors_have_fixed_private_free_display() {
        let raw_protocol_error = agent_client_protocol::Error::new(
            -32603,
            "C:/Users/ExampleUser/private xai-secret-value",
        )
        .data(json!({ "token": "another-private-token" }));
        let protocol_error = ProbeError::from(raw_protocol_error);
        let protocol_display = protocol_error.to_string();
        assert_eq!(
            protocol_display,
            "ACP protocol operation failed (internal_error)"
        );
        assert!(std::error::Error::source(&protocol_error).is_none());

        let invalid_workspace = ProbeError::InvalidWorkspace;
        assert_eq!(
            invalid_workspace.to_string(),
            "workspace is not an existing directory"
        );

        let canonicalize_error = ProbeError::CanonicalizeWorkspace;
        assert_eq!(
            canonicalize_error.to_string(),
            "failed to canonicalize workspace"
        );

        for displayed in [
            protocol_display,
            invalid_workspace.to_string(),
            canonicalize_error.to_string(),
        ] {
            for private_value in ["ExampleUser", "private", "xai-secret-value", "token"] {
                assert!(!displayed.contains(private_value));
            }
        }
    }

    fn auth_method(id: &str, name: &str) -> AuthMethod {
        AuthMethod::Agent(AuthMethodAgent::new(id.to_owned(), name.to_owned()))
    }
}
