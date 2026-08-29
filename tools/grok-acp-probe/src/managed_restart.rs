use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, CloseSessionRequest, InitializeRequest, ListSessionsRequest,
    NewSessionRequest, ResumeSessionRequest, SessionId,
};
use agent_client_protocol::{Agent, ConnectionTo};
use grok_runtime::{ProcessDiagnostics, WindowsAcpProcess};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::{
    ACP_SDK_VERSION, ManagedProcessSummary, ProbeError, ProbeTarget, StepObservation,
    canonicalize_workspace, choose_auth_method, probe_client_capabilities,
};

const CLEANUP_RESERVE: Duration = Duration::from_secs(5);
const MAX_SESSION_LIST_PAGES: usize = 128;
const MAX_SESSION_CURSOR_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRestartReport {
    pub sdk_version: &'static str,
    pub wire_protocol: u16,
    pub steps: Vec<StepObservation>,
    pub first_process: ManagedProcessSummary,
    pub second_process: ManagedProcessSummary,
}

struct CreatedSession {
    id: SessionId,
    steps: Vec<StepObservation>,
}

/// Proves that an ACP session survives an abrupt, Job Object-contained process
/// teardown and can be discovered and resumed by a fresh managed process.
///
/// The session identifier remains internal and is never included in the report.
pub async fn run_managed_restart_probe(
    target: ProbeTarget,
    workspace: PathBuf,
    requested_auth_method: Option<String>,
    deadline: Duration,
) -> Result<ManagedRestartReport, ProbeError> {
    let workspace = canonicalize_workspace(workspace)?;
    let expires = Instant::now() + deadline;

    let first_process = managed_process(&target);
    let first_diagnostics = first_process.diagnostics();
    let (created_tx, created_rx) = oneshot::channel();
    let first_workspace = workspace.clone();
    let first_auth_method = requested_auth_method.clone();
    let first_connection = agent_client_protocol::Client
        .builder()
        .name("grok-build-gui-phase-0-managed-restart-first")
        .connect_with(
            first_process,
            move |connection: ConnectionTo<Agent>| async move {
                let result = create_zero_turn_session(
                    &connection,
                    first_workspace,
                    first_auth_method.as_deref(),
                )
                .await;
                let _ = created_tx.send(result);
                std::future::pending::<Result<(), agent_client_protocol::Error>>().await
            },
        );
    let mut first_task = tokio::spawn(first_connection);

    // Do not use `?` until the managed task has been aborted and awaited. This
    // guarantees every first-phase error path tears down its Job Object.
    let created = tokio::time::timeout_at(expires, created_rx).await;
    first_task.abort();
    let _ = (&mut first_task).await;
    let created = created
        .map_err(|_| ProbeError::Timeout(deadline))?
        .map_err(|_| ProbeError::ManagedRestart("first_connection_ended"))??;

    let mut steps = created.steps;
    steps.push(StepObservation::confirmed(
        "first/process_aborted_and_awaited",
    ));
    let first_process = process_summary(&first_diagnostics);

    let second_process = managed_process(&target);
    let second_diagnostics = second_process.diagnostics();
    let session_id = created.id;
    let second_connection = agent_client_protocol::Client
        .builder()
        .name("grok-build-gui-phase-0-managed-restart-second")
        .connect_with(
            second_process,
            move |connection: ConnectionTo<Agent>| async move {
                Ok(recover_and_close_session(
                    &connection,
                    workspace,
                    session_id,
                    requested_auth_method.as_deref(),
                    expires,
                )
                .await)
            },
        );
    let mut second_steps = tokio::time::timeout_at(expires, second_connection)
        .await
        .map_err(|_| ProbeError::Timeout(deadline))?
        .map_err(|_| ProbeError::ManagedRestart("second_connection_failed"))??;
    steps.append(&mut second_steps);

    Ok(ManagedRestartReport {
        sdk_version: ACP_SDK_VERSION,
        wire_protocol: 1,
        steps,
        first_process,
        second_process: process_summary(&second_diagnostics),
    })
}

async fn create_zero_turn_session(
    connection: &ConnectionTo<Agent>,
    workspace: PathBuf,
    requested_auth_method: Option<&str>,
) -> Result<CreatedSession, ProbeError> {
    let (initialize, authenticated) = initialize_and_authenticate(
        connection,
        requested_auth_method,
        "first_initialize_request_failed",
        "first_authenticate_request_failed",
    )
    .await?;
    require_recovery_capabilities(&initialize)?;
    let session = connection
        .send_request(NewSessionRequest::new(workspace))
        .block_task()
        .await
        .map_err(|_| ProbeError::ManagedRestart("first_session_new_request_failed"))?;
    Ok(CreatedSession {
        id: session.session_id,
        steps: vec![
            StepObservation::confirmed("first/windows_job_object_containment"),
            StepObservation::confirmed("first/initialize"),
            auth_step("first/authenticate", authenticated),
            StepObservation::confirmed("first/session_new_zero_turn"),
        ],
    })
}

async fn recover_and_close_session(
    connection: &ConnectionTo<Agent>,
    workspace: PathBuf,
    session_id: SessionId,
    requested_auth_method: Option<&str>,
    expires: Instant,
) -> Result<Vec<StepObservation>, ProbeError> {
    let (initialize, authenticated) = initialize_and_authenticate(
        connection,
        requested_auth_method,
        "second_initialize_request_failed",
        "second_authenticate_request_failed",
    )
    .await?;
    let mut steps = vec![
        StepObservation::confirmed("second/windows_job_object_containment"),
        StepObservation::confirmed("second/initialize"),
        auth_step("second/authenticate", authenticated),
    ];
    let capabilities = &initialize.agent_capabilities.session_capabilities;
    if capabilities.close.is_none() {
        return Err(ProbeError::ManagedRestart("session_close_not_advertised"));
    }

    // Once the probe has created a persisted session, always attempt to close
    // it after the second process initializes, even when discovery or resume
    // fails. A close failure takes precedence because cleanup is then unknown.
    let now = Instant::now();
    let recovery_expires = now
        + expires
            .saturating_duration_since(now)
            .saturating_sub(CLEANUP_RESERVE);
    let recovery = tokio::time::timeout_at(
        recovery_expires,
        recover_session(connection, capabilities, workspace, session_id.clone()),
    )
    .await
    .map_err(|_| ProbeError::ManagedRestart("session_recovery_timed_out"))
    .and_then(std::convert::identity);
    let close = tokio::time::timeout_at(
        expires,
        connection
            .send_request(CloseSessionRequest::new(session_id))
            .block_task(),
    )
    .await
    .map_err(|_| ProbeError::ManagedRestart("session_close_timed_out"))?
    .map_err(|_| ProbeError::ManagedRestart("session_close_request_failed"));
    close?;
    recovery.map(|mut recovered_steps| {
        steps.append(&mut recovered_steps);
        steps.push(StepObservation::confirmed("second/session_close"));
        steps
    })
}

async fn recover_session(
    connection: &ConnectionTo<Agent>,
    capabilities: &agent_client_protocol::schema::v1::SessionCapabilities,
    workspace: PathBuf,
    session_id: SessionId,
) -> Result<Vec<StepObservation>, ProbeError> {
    if capabilities.list.is_none() {
        return Err(ProbeError::ManagedRestart("session_list_not_advertised"));
    }
    if capabilities.resume.is_none() {
        return Err(ProbeError::ManagedRestart("session_resume_not_advertised"));
    }

    let (sessions_scanned, pages_scanned) =
        find_exact_session(connection, &workspace, &session_id).await?;

    connection
        .send_request(ResumeSessionRequest::new(session_id, workspace))
        .block_task()
        .await
        .map_err(|_| ProbeError::ManagedRestart("session_resume_request_failed"))?;

    Ok(vec![
        StepObservation::confirmed_with(
            "second/session_list_exact_match",
            json!({
                "sessionsScanned": sessions_scanned,
                "pagesScanned": pages_scanned,
                "exactSessionFound": true
            }),
        ),
        StepObservation::confirmed("second/session_resume"),
    ])
}

async fn find_exact_session(
    connection: &ConnectionTo<Agent>,
    workspace: &std::path::Path,
    session_id: &SessionId,
) -> Result<(usize, usize), ProbeError> {
    let mut cursor = None;
    let mut seen_cursors = BTreeSet::new();
    let mut sessions_scanned = 0_usize;

    for page_index in 0..MAX_SESSION_LIST_PAGES {
        let response = connection
            .send_request(
                ListSessionsRequest::new()
                    .cwd(Some(workspace.to_path_buf()))
                    .cursor(cursor),
            )
            .block_task()
            .await
            .map_err(|_| ProbeError::ManagedRestart("session_list_request_failed"))?;
        sessions_scanned = sessions_scanned.saturating_add(response.sessions.len());
        if response
            .sessions
            .iter()
            .any(|session| &session.session_id == session_id)
        {
            return Ok((sessions_scanned, page_index + 1));
        }

        let Some(next_cursor) = response.next_cursor else {
            return Err(ProbeError::ManagedRestart("exact_session_not_found"));
        };
        if next_cursor.len() > MAX_SESSION_CURSOR_BYTES {
            return Err(ProbeError::ManagedRestart("session_list_cursor_too_large"));
        }
        if !seen_cursors.insert(next_cursor.clone()) {
            return Err(ProbeError::ManagedRestart("session_list_cursor_repeated"));
        }
        cursor = Some(next_cursor);
    }

    Err(ProbeError::ManagedRestart(
        "session_list_page_limit_exceeded",
    ))
}

fn require_recovery_capabilities(
    initialize: &agent_client_protocol::schema::v1::InitializeResponse,
) -> Result<(), ProbeError> {
    let capabilities = &initialize.agent_capabilities.session_capabilities;
    if capabilities.list.is_none() {
        return Err(ProbeError::ManagedRestart("session_list_not_advertised"));
    }
    if capabilities.resume.is_none() {
        return Err(ProbeError::ManagedRestart("session_resume_not_advertised"));
    }
    if capabilities.close.is_none() {
        return Err(ProbeError::ManagedRestart("session_close_not_advertised"));
    }
    Ok(())
}

async fn initialize_and_authenticate(
    connection: &ConnectionTo<Agent>,
    requested_auth_method: Option<&str>,
    initialize_error: &'static str,
    authenticate_error: &'static str,
) -> Result<(agent_client_protocol::schema::v1::InitializeResponse, bool), ProbeError> {
    let initialize = connection
        .send_request(
            InitializeRequest::new(ProtocolVersion::V1)
                .client_capabilities(probe_client_capabilities()),
        )
        .block_task()
        .await
        .map_err(|_| ProbeError::ManagedRestart(initialize_error))?;
    let selected_auth_method = choose_auth_method(&initialize, requested_auth_method);
    if selected_auth_method.is_none() && !initialize.auth_methods.is_empty() {
        return Err(ProbeError::ManagedRestart("no_supported_auth_method"));
    }
    let authenticated = if let Some(method) = selected_auth_method {
        let mut meta = Map::new();
        meta.insert("headless".to_owned(), Value::Bool(true));
        connection
            .send_request(AuthenticateRequest::new(method).meta(meta))
            .block_task()
            .await
            .map_err(|_| ProbeError::ManagedRestart(authenticate_error))?;
        true
    } else {
        false
    };
    Ok((initialize, authenticated))
}

fn managed_process(target: &ProbeTarget) -> WindowsAcpProcess {
    WindowsAcpProcess::new(target.executable.clone())
        .args(target.arguments.clone())
        .envs(target.environment.clone())
}

fn process_summary(diagnostics: &ProcessDiagnostics) -> ManagedProcessSummary {
    let snapshot = diagnostics.snapshot();
    ManagedProcessSummary {
        containment: "suspended_then_job_assigned",
        stderr_line_count: snapshot.total_lines,
        stderr_byte_count: snapshot.total_bytes,
        stderr_truncated_line_count: snapshot.truncated_lines,
    }
}

fn auth_step(step: &'static str, advertised: bool) -> StepObservation {
    if advertised {
        StepObservation::confirmed(step)
    } else {
        StepObservation::skipped(step, "no_auth_method_advertised")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_shape_has_no_session_identifier_field() {
        let report = ManagedRestartReport {
            sdk_version: ACP_SDK_VERSION,
            wire_protocol: 1,
            steps: vec![StepObservation::confirmed_with(
                "second/session_list_exact_match",
                json!({ "sessionCount": 1, "exactSessionFound": true }),
            )],
            first_process: process_counts(),
            second_process: process_counts(),
        };
        let encoded = serde_json::to_string(&report).expect("serialize report");
        assert!(!encoded.contains("sessionId"));
        assert!(!encoded.contains("session_id"));
    }

    fn process_counts() -> ManagedProcessSummary {
        ManagedProcessSummary {
            containment: "suspended_then_job_assigned",
            stderr_line_count: 0,
            stderr_byte_count: 0,
            stderr_truncated_line_count: 0,
        }
    }
}
