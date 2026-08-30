mod application_contract;

use std::{sync::Arc, time::Duration};

use application_contract::{
    APPLICATION_EVENT_NAME, AcknowledgementDto, ApplicationErrorDto, ApplicationEvent,
    ApplicationEventClock, ElicitationResponseRequestDto, InteractionKindDto,
    ListSessionsRequestDto, NewSessionRequestDto, PermissionResponseRequestDto, PromptRequestDto,
    PromptResultDto, RuntimeSnapshotDto, SessionDto, SessionPageDto, SessionRequestDto,
    SessionWorkspaceRequestDto, SetSessionConfigRequestDto, SetSessionModeRequestDto,
    SetSessionModelRequestDto, SetupStatusDto, WorkspaceDto, WorkspaceRequestDto,
    acknowledgement_from_response, prompt_from_response, session_from_response,
    sessions_from_response,
};
use grok_runtime::{GrokRuntime, RedactedDiagnostic, RuntimeError, RuntimeEvent, RuntimeState};
use tauri::Emitter as _;

struct RuntimeManager {
    runtime: Result<GrokRuntime, RuntimeError>,
    event_clock: Arc<ApplicationEventClock>,
    lifecycle: tokio::sync::Mutex<()>,
}

impl RuntimeManager {
    fn discover() -> Self {
        Self {
            runtime: GrokRuntime::discover(None),
            event_clock: Arc::new(ApplicationEventClock::default()),
            lifecycle: tokio::sync::Mutex::new(()),
        }
    }

    fn runtime(&self) -> Result<&GrokRuntime, ApplicationErrorDto> {
        self.runtime
            .as_ref()
            .map_err(Clone::clone)
            .map_err(ApplicationErrorDto::from)
    }

    fn snapshot(&self) -> Result<RuntimeSnapshotDto, ApplicationErrorDto> {
        let checkpoint = self.event_clock.checkpoint();
        Ok(RuntimeSnapshotDto::from_runtime(
            self.runtime()?.snapshot(),
            checkpoint,
        ))
    }

    async fn synchronized_snapshot(
        &self,
        minimum_generation: u64,
    ) -> Result<RuntimeSnapshotDto, ApplicationErrorDto> {
        let runtime = self.runtime()?;
        let expected_state = runtime.snapshot().state.into();
        let checkpoint = tokio::time::timeout(
            Duration::from_secs(5),
            self.event_clock
                .wait_for_runtime_state(minimum_generation, expected_state),
        )
        .await
        .map_err(|_| ApplicationErrorDto::event_delivery_timeout())?;
        Ok(RuntimeSnapshotDto::from_runtime(
            runtime.snapshot(),
            checkpoint,
        ))
    }
}

fn emit_application_event(
    app: &tauri::AppHandle,
    event_clock: &ApplicationEventClock,
    event: RuntimeEvent,
) {
    let envelope = event_clock.envelope_runtime(event);
    let _ = app.emit(APPLICATION_EVENT_NAME, envelope);
}

fn emit_local_application_event(
    app: &tauri::AppHandle,
    event_clock: &ApplicationEventClock,
    event: ApplicationEvent,
) {
    let envelope = event_clock.envelope(event);
    let _ = app.emit(APPLICATION_EVENT_NAME, envelope);
}

#[tauri::command]
fn setup_status(manager: tauri::State<'_, RuntimeManager>) -> SetupStatusDto {
    match manager.runtime.as_ref() {
        Ok(_) => SetupStatusDto {
            runtime_available: true,
            failure: None,
        },
        Err(error) => SetupStatusDto {
            runtime_available: false,
            failure: Some(error.clone().into()),
        },
    }
}

#[tauri::command]
fn workspace_validate(request: WorkspaceRequestDto) -> Result<WorkspaceDto, ApplicationErrorDto> {
    WorkspaceDto::validate(request)
}

#[tauri::command]
fn runtime_snapshot(
    manager: tauri::State<'_, RuntimeManager>,
) -> Result<RuntimeSnapshotDto, ApplicationErrorDto> {
    manager.snapshot()
}

#[tauri::command]
async fn runtime_start(
    manager: tauri::State<'_, RuntimeManager>,
) -> Result<RuntimeSnapshotDto, ApplicationErrorDto> {
    let _lifecycle = manager.lifecycle.lock().await;
    let runtime = manager.runtime()?;
    let checkpoint = manager.event_clock.checkpoint();
    let starts_generation = matches!(
        runtime.snapshot().state,
        RuntimeState::Disconnected | RuntimeState::Failed
    );
    runtime.start().await.map_err(ApplicationErrorDto::from)?;
    let minimum_generation = checkpoint.generation + u64::from(starts_generation);
    manager.synchronized_snapshot(minimum_generation).await
}

#[tauri::command]
async fn runtime_stop(
    manager: tauri::State<'_, RuntimeManager>,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    let _lifecycle = manager.lifecycle.lock().await;
    manager
        .runtime()?
        .stop()
        .await
        .map_err(ApplicationErrorDto::from)?;
    Ok(AcknowledgementDto::accepted())
}

#[tauri::command]
async fn runtime_restart(
    manager: tauri::State<'_, RuntimeManager>,
) -> Result<RuntimeSnapshotDto, ApplicationErrorDto> {
    let _lifecycle = manager.lifecycle.lock().await;
    let minimum_generation = manager
        .event_clock
        .checkpoint()
        .generation
        .checked_add(1)
        .ok_or_else(ApplicationErrorDto::event_delivery_timeout)?;
    manager
        .runtime()?
        .restart()
        .await
        .map_err(ApplicationErrorDto::from)?;
    manager.synchronized_snapshot(minimum_generation).await
}

#[tauri::command]
async fn session_new(
    manager: tauri::State<'_, RuntimeManager>,
    request: NewSessionRequestDto,
) -> Result<SessionDto, ApplicationErrorDto> {
    let response = manager
        .runtime()?
        .execute(request.into_runtime_command()?)
        .await
        .map_err(ApplicationErrorDto::from)?;
    session_from_response(response)
}

#[tauri::command]
async fn session_list(
    manager: tauri::State<'_, RuntimeManager>,
    request: ListSessionsRequestDto,
) -> Result<SessionPageDto, ApplicationErrorDto> {
    let response = manager
        .runtime()?
        .execute(request.into_runtime_command()?)
        .await
        .map_err(ApplicationErrorDto::from)?;
    sessions_from_response(response)
}

#[tauri::command]
async fn session_load(
    manager: tauri::State<'_, RuntimeManager>,
    request: SessionWorkspaceRequestDto,
) -> Result<SessionDto, ApplicationErrorDto> {
    let command = request.into_load_command()?;
    let response = manager
        .runtime()?
        .execute(command)
        .await
        .map_err(ApplicationErrorDto::from)?;
    session_from_response(response)
}

#[tauri::command]
async fn session_resume(
    manager: tauri::State<'_, RuntimeManager>,
    request: SessionWorkspaceRequestDto,
) -> Result<SessionDto, ApplicationErrorDto> {
    let command = request.into_resume_command()?;
    let response = manager
        .runtime()?
        .execute(command)
        .await
        .map_err(ApplicationErrorDto::from)?;
    session_from_response(response)
}

#[tauri::command]
async fn session_close(
    manager: tauri::State<'_, RuntimeManager>,
    request: SessionRequestDto,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    let response = manager
        .runtime()?
        .execute(grok_runtime::RuntimeCommand::CloseSession {
            session_id: request.validated_session_id()?,
        })
        .await
        .map_err(ApplicationErrorDto::from)?;
    acknowledgement_from_response(response)
}

#[tauri::command]
async fn prompt_send(
    manager: tauri::State<'_, RuntimeManager>,
    request: PromptRequestDto,
) -> Result<PromptResultDto, ApplicationErrorDto> {
    let response = manager
        .runtime()?
        .execute(request.into_runtime_command()?)
        .await
        .map_err(ApplicationErrorDto::from)?;
    prompt_from_response(response)
}

#[tauri::command]
async fn prompt_cancel(
    manager: tauri::State<'_, RuntimeManager>,
    request: SessionRequestDto,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    let response = manager
        .runtime()?
        .execute(grok_runtime::RuntimeCommand::Cancel {
            session_id: request.validated_session_id()?,
        })
        .await
        .map_err(ApplicationErrorDto::from)?;
    acknowledgement_from_response(response)
}

#[tauri::command]
async fn session_set_mode(
    manager: tauri::State<'_, RuntimeManager>,
    request: SetSessionModeRequestDto,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    let response = manager
        .runtime()?
        .execute(request.into_runtime_command()?)
        .await
        .map_err(ApplicationErrorDto::from)?;
    acknowledgement_from_response(response)
}

#[tauri::command]
async fn session_set_model(
    manager: tauri::State<'_, RuntimeManager>,
    request: SetSessionModelRequestDto,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    let response = manager
        .runtime()?
        .execute(request.into_runtime_command()?)
        .await
        .map_err(ApplicationErrorDto::from)?;
    acknowledgement_from_response(response)
}

#[tauri::command]
async fn session_set_config(
    manager: tauri::State<'_, RuntimeManager>,
    request: SetSessionConfigRequestDto,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    let response = manager
        .runtime()?
        .execute(request.into_runtime_command()?)
        .await
        .map_err(ApplicationErrorDto::from)?;
    acknowledgement_from_response(response)
}

#[tauri::command]
async fn permission_respond(
    app: tauri::AppHandle,
    manager: tauri::State<'_, RuntimeManager>,
    request: PermissionResponseRequestDto,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    request.validate()?;
    manager
        .runtime()?
        .respond_permission(&request.interaction_id, request.decision.into())
        .await
        .map_err(ApplicationErrorDto::from)?;
    emit_local_application_event(
        &app,
        &manager.event_clock,
        ApplicationEvent::InteractionResolved {
            interaction_id: request.interaction_id,
            kind: InteractionKindDto::Permission,
        },
    );
    Ok(AcknowledgementDto::accepted())
}

#[tauri::command]
async fn elicitation_respond(
    app: tauri::AppHandle,
    manager: tauri::State<'_, RuntimeManager>,
    request: ElicitationResponseRequestDto,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    request.validate()?;
    manager
        .runtime()?
        .respond_elicitation(&request.interaction_id, request.decision.into())
        .await
        .map_err(ApplicationErrorDto::from)?;
    emit_local_application_event(
        &app,
        &manager.event_clock,
        ApplicationEvent::InteractionResolved {
            interaction_id: request.interaction_id,
            kind: InteractionKindDto::Elicitation,
        },
    );
    Ok(AcknowledgementDto::accepted())
}

pub fn run() {
    let manager = RuntimeManager::discover();
    let event_source = manager.runtime.as_ref().ok().cloned();
    let event_clock = Arc::clone(&manager.event_clock);

    tauri::Builder::default()
        .manage(manager)
        .setup(move |app| {
            if let Some(runtime) = event_source {
                let mut events = runtime.subscribe();
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        match events.recv().await {
                            Ok(event) => {
                                emit_application_event(&app_handle, &event_clock, event);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                emit_application_event(
                                    &app_handle,
                                    &event_clock,
                                    RuntimeEvent::RuntimeFailed {
                                        diagnostic: RedactedDiagnostic::new(
                                            "runtime event delivery fell behind; refresh required",
                                        ),
                                        recoverable: true,
                                    },
                                );
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            setup_status,
            workspace_validate,
            runtime_snapshot,
            runtime_start,
            runtime_stop,
            runtime_restart,
            session_new,
            session_list,
            session_load,
            session_resume,
            session_close,
            prompt_send,
            prompt_cancel,
            session_set_mode,
            session_set_model,
            session_set_config,
            permission_respond,
            elicitation_respond,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Grok Build GUI desktop shell");
}
