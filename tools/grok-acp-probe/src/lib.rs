use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthCapabilities, AuthenticateRequest, CancelNotification, CloseSessionRequest, ContentBlock,
    CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationCapabilities, ElicitationFormCapabilities, ElicitationUrlCapabilities,
    InitializeRequest, InitializeResponse, ListSessionsRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ResumeSessionRequest, SelectedPermissionOutcome,
    SessionNotification, SessionUpdate, TextContent,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, LineDirection};
use serde::Serialize;
use serde_json::{Map, Value, json};
use thiserror::Error;

pub const ACP_SDK_VERSION: &str = "2.0.0";

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

    fn process_config(&self) -> AcpAgentConfig {
        AcpAgentConfig::new(self.executable.clone())
            .args(self.arguments.clone())
            .envs(self.environment.clone())
    }

    fn summary(&self) -> TargetSummary {
        TargetSummary {
            executable: self.executable.file_name().map_or_else(
                || "<unknown>".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            ),
            arguments: self.arguments.clone(),
            overridden_environment_keys: self.environment.keys().cloned().collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ProbeKind {
    InitializeOnly,
    Lifecycle(LifecycleOptions),
}

#[derive(Clone, Debug)]
pub struct LifecycleOptions {
    pub workspace: PathBuf,
    pub prompt: String,
    pub auth_method: Option<String>,
    pub exercise_cancel: bool,
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
    pub request_shapes: BTreeMap<String, BTreeSet<String>>,
    pub wire: WireSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSummary {
    pub executable: String,
    pub arguments: Vec<String>,
    pub overridden_environment_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeSummary {
    pub protocol_version: Value,
    pub agent_info: Option<Value>,
    pub agent_capabilities: Value,
    pub auth_methods: Vec<AuthMethodSummary>,
    pub metadata_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethodSummary {
    pub id: String,
    pub name: String,
    pub description_present: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepObservation {
    pub step: String,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
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

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireSummary {
    pub client_methods: BTreeSet<String>,
    pub agent_methods: BTreeSet<String>,
    pub client_response_count: u64,
    pub agent_response_count: u64,
    pub malformed_stdout_lines: u64,
    pub stderr_line_count: u64,
    pub stderr_byte_count: u64,
}

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("ACP probe exceeded its {0:?} deadline")]
    Timeout(Duration),
    #[error("ACP connection failed: {0}")]
    Protocol(#[from] agent_client_protocol::Error),
    #[error("workspace is not an existing directory: {0}")]
    InvalidWorkspace(PathBuf),
    #[error("failed to canonicalize workspace {path}: {source}")]
    CanonicalizeWorkspace {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Default)]
struct EventCollector {
    counts: BTreeMap<String, u64>,
    request_shapes: BTreeMap<String, BTreeSet<String>>,
}

impl EventCollector {
    fn increment(&mut self, event: &'static str) {
        *self.counts.entry(event.to_owned()).or_default() += 1;
    }

    fn record_request_shape<T: Serialize>(&mut self, method: &'static str, request: &T) {
        let Some(value) = to_safe_value(request) else {
            return;
        };
        let fields = self.request_shapes.entry(method.to_owned()).or_default();
        collect_field_paths(&value, "", fields);
    }
}

pub async fn run_probe(
    target: ProbeTarget,
    kind: ProbeKind,
    deadline: Duration,
) -> Result<ProbeReport, ProbeError> {
    let kind = match kind {
        ProbeKind::InitializeOnly => ProbeKind::InitializeOnly,
        ProbeKind::Lifecycle(mut options) => {
            if !options.workspace.is_dir() {
                return Err(ProbeError::InvalidWorkspace(options.workspace));
            }
            let original = options.workspace.clone();
            options.workspace = std::fs::canonicalize(&options.workspace).map_err(|source| {
                ProbeError::CanonicalizeWorkspace {
                    path: original,
                    source,
                }
            })?;
            ProbeKind::Lifecycle(options)
        }
    };

    let wire = Arc::new(Mutex::new(WireSummary::default()));
    let events = Arc::new(Mutex::new(EventCollector::default()));
    let wire_for_debug = Arc::clone(&wire);
    let agent = AcpAgent::new(target.process_config()).with_debug(move |line, direction| {
        collect_wire_summary(&wire_for_debug, line, direction);
    });

    let events_for_notification = Arc::clone(&events);
    let events_for_permission = Arc::clone(&events);
    let events_for_elicitation = Arc::clone(&events);
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
                    events.record_request_shape("session/request_permission", &request);
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
                    events.record_request_shape("elicitation/create", &_request);
                }
                responder.respond(CreateElicitationResponse::new(ElicitationAction::Cancel))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| async move {
            let capabilities = agent_client_protocol::schema::v1::ClientCapabilities::new()
                .auth(AuthCapabilities::new().terminal(false))
                .elicitation(Some(
                    ElicitationCapabilities::new()
                        .form(Some(ElicitationFormCapabilities::new()))
                        .url(Some(ElicitationUrlCapabilities::new())),
                ));
            let initialize = connection
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_capabilities(capabilities),
                )
                .block_task()
                .await?;
            let initialize_summary = summarize_initialize(&initialize);
            let mut steps = vec![StepObservation::confirmed("initialize")];

            if let ProbeKind::Lifecycle(options) = kind {
                let auth_method = choose_auth_method(&initialize, options.auth_method.as_deref());
                if let Some(method_id) = auth_method {
                    let mut meta = Map::new();
                    meta.insert("headless".to_owned(), Value::Bool(true));
                    connection
                        .send_request(AuthenticateRequest::new(method_id.clone()).meta(meta))
                        .block_task()
                        .await?;
                    steps.push(StepObservation::confirmed_with(
                        "authenticate",
                        json!({ "methodId": method_id }),
                    ));
                } else {
                    steps.push(StepObservation::skipped(
                        "authenticate",
                        "agent_advertised_no_auth_methods",
                    ));
                }

                let new_session = connection
                    .send_request(NewSessionRequest::new(options.workspace.clone()))
                    .block_task()
                    .await?;
                let session_id = new_session.session_id;
                steps.push(StepObservation::confirmed_with(
                    "session/new",
                    json!({
                        "modesPresent": new_session.modes.is_some(),
                        "configOptionCount": new_session.config_options.as_ref().map_or(0, Vec::len),
                    }),
                ));

                let prompt_response = connection
                    .send_request(PromptRequest::new(
                        session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(options.prompt))],
                    ))
                    .block_task()
                    .await?;
                steps.push(StepObservation::confirmed_with(
                    "session/prompt",
                    json!({ "stopReason": prompt_response.stop_reason }),
                ));

                if options.exercise_cancel {
                    let pending = connection.send_request(PromptRequest::new(
                        session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(
                            "Wait until cancelled. Do not use tools or modify files.",
                        ))],
                    ));
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    connection.send_notification(CancelNotification::new(session_id.clone()))?;
                    let cancelled = pending.block_task().await?;
                    steps.push(StepObservation::confirmed_with(
                        "session/cancel",
                        json!({ "stopReason": cancelled.stop_reason }),
                    ));
                } else {
                    steps.push(StepObservation::skipped("session/cancel", "not_requested"));
                }

                let session_capabilities = &initialize.agent_capabilities.session_capabilities;
                if session_capabilities.list.is_some() {
                    match connection
                        .send_request(ListSessionsRequest::new().cwd(Some(options.workspace.clone())))
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
                        Err(_) => steps.push(StepObservation::failed("session/list")),
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
                            options.workspace.clone(),
                        ))
                        .block_task()
                        .await
                    {
                        Ok(_) => steps.push(StepObservation::confirmed("session/load")),
                        Err(_) => steps.push(StepObservation::failed("session/load")),
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
                            options.workspace.clone(),
                        ))
                        .block_task()
                        .await
                    {
                        Ok(_) => steps.push(StepObservation::confirmed("session/resume")),
                        Err(_) => steps.push(StepObservation::failed("session/resume")),
                    }
                } else {
                    steps.push(StepObservation::skipped(
                        "session/resume",
                        "capability_not_advertised",
                    ));
                }

                if session_capabilities.close.is_some() {
                    match connection
                        .send_request(CloseSessionRequest::new(session_id))
                        .block_task()
                        .await
                    {
                        Ok(_) => steps.push(StepObservation::confirmed("session/close")),
                        Err(_) => steps.push(StepObservation::failed("session/close")),
                    }
                } else {
                    steps.push(StepObservation::skipped(
                        "session/close",
                        "capability_not_advertised",
                    ));
                }
            }

            let (event_counts, request_shapes) = {
                let events = lock_unpoisoned(&events);
                (events.counts.clone(), events.request_shapes.clone())
            };
            let wire = lock_unpoisoned(&wire).clone();

            Ok(ProbeReport {
                sdk_version: ACP_SDK_VERSION,
                wire_protocol: 1,
                target: target_summary,
                initialize: initialize_summary,
                steps,
                event_counts,
                request_shapes,
                wire,
            })
        });

    tokio::time::timeout(deadline, connection)
        .await
        .map_err(|_| ProbeError::Timeout(deadline))?
        .map_err(ProbeError::from)
}

fn choose_auth_method(initialize: &InitializeResponse, requested: Option<&str>) -> Option<String> {
    if let Some(requested) = requested {
        return initialize
            .auth_methods
            .iter()
            .find(|method| method.id().to_string() == requested)
            .map(|method| method.id().to_string());
    }

    const PREFERRED: &[&str] = &["cached_token", "grok", "xai.api_key"];
    PREFERRED
        .iter()
        .find_map(|preferred| {
            initialize
                .auth_methods
                .iter()
                .find(|method| method.id().to_string() == *preferred)
        })
        .or_else(|| initialize.auth_methods.first())
        .map(|method| method.id().to_string())
}

fn summarize_initialize(response: &InitializeResponse) -> InitializeSummary {
    let mut agent_info = response.agent_info.as_ref().and_then(to_safe_value);
    if let Some(value) = agent_info.as_mut() {
        strip_metadata(value);
    }
    let mut agent_capabilities =
        to_safe_value(&response.agent_capabilities).unwrap_or_else(|| Value::Object(Map::new()));
    strip_metadata(&mut agent_capabilities);

    InitializeSummary {
        protocol_version: to_safe_value(&response.protocol_version).unwrap_or(Value::Null),
        agent_info,
        agent_capabilities,
        auth_methods: response
            .auth_methods
            .iter()
            .map(|method| AuthMethodSummary {
                id: method.id().to_string(),
                name: method.name().to_owned(),
                description_present: method.description().is_some(),
            })
            .collect(),
        metadata_keys: response
            .meta
            .as_ref()
            .map_or_else(Vec::new, |meta| meta.keys().cloned().collect()),
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

fn collect_field_paths(value: &Value, prefix: &str, fields: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                fields.insert(path.clone());
                collect_field_paths(child, &path, fields);
            }
        }
        Value::Array(values) => {
            let path = format!("{prefix}[]");
            fields.insert(path.clone());
            for child in values {
                collect_field_paths(child, &path, fields);
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

fn collect_wire_summary(summary: &Arc<Mutex<WireSummary>>, line: &str, direction: LineDirection) {
    let mut summary = lock_unpoisoned(summary);
    if direction == LineDirection::Stderr {
        summary.stderr_line_count += 1;
        summary.stderr_byte_count += line.len() as u64;
        return;
    }

    let Ok(value) = serde_json::from_str::<Value>(line) else {
        if direction == LineDirection::Stdout {
            summary.malformed_stdout_lines += 1;
        }
        return;
    };
    collect_wire_value(&mut summary, &value, direction);
}

fn collect_wire_value(summary: &mut WireSummary, value: &Value, direction: LineDirection) {
    if let Value::Array(values) = value {
        for value in values {
            collect_wire_value(summary, value, direction);
        }
        return;
    }

    let Some(object) = value.as_object() else {
        return;
    };
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        match direction {
            LineDirection::Stdin => {
                summary.client_methods.insert(method.to_owned());
            }
            LineDirection::Stdout => {
                summary.agent_methods.insert(method.to_owned());
            }
            LineDirection::Stderr => {}
        }
    } else if object.contains_key("result") || object.contains_key("error") {
        match direction {
            LineDirection::Stdin => summary.client_response_count += 1,
            LineDirection::Stdout => summary.agent_response_count += 1,
            LineDirection::Stderr => {}
        }
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
    fn wire_summary_records_shape_but_not_payloads() {
        let summary = Arc::new(Mutex::new(WireSummary::default()));
        collect_wire_summary(
            &summary,
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"private":"text"}}"#,
            LineDirection::Stdout,
        );
        collect_wire_summary(
            &summary,
            r#"{"jsonrpc":"2.0","id":1,"result":{"private":"text"}}"#,
            LineDirection::Stdout,
        );
        let summary = lock_unpoisoned(&summary);
        assert_eq!(
            summary.agent_methods,
            BTreeSet::from(["session/update".to_owned()])
        );
        assert_eq!(summary.agent_response_count, 1);
    }

    #[test]
    fn request_shape_records_only_field_names() {
        let mut collector = EventCollector::default();
        collector.record_request_shape(
            "fixture/request",
            &json!({
                "sessionId": "private-session",
                "options": [{ "optionId": "private-option", "kind": "reject_once" }],
            }),
        );

        let shape = &collector.request_shapes["fixture/request"];
        assert!(shape.contains("sessionId"));
        assert!(shape.contains("options[]"));
        assert!(shape.contains("options[].optionId"));
        assert!(!shape.iter().any(|field| field.contains("private")));
    }
}
