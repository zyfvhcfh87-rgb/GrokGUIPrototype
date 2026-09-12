mod application_contract;
mod presentation;
mod workspace;
mod workspace_changes;

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use application_contract::{
    APPLICATION_EVENT_NAME, AcknowledgementDto, ApplicationErrorDto, ApplicationEvent,
    ApplicationEventClock, ElicitationResponseRequestDto, ExecutableSourceDto, ExecutableStateDto,
    InteractionKindDto, ListSessionsRequestDto, NewSessionRequestDto, OpenExternalUrlRequestDto,
    PermissionResponseRequestDto, PresentationPreferencesDto, PromptRequestDto, PromptResultDto,
    RecentWorkspaceListDto, RuntimeDiagnosticsDto, RuntimeSnapshotDto, SessionDto, SessionPageDto,
    SessionRequestDto, SessionWorkspaceRequestDto, SetSessionConfigRequestDto,
    SetSessionModeRequestDto, SetSessionModelRequestDto, SetupStatusDto, WorkspaceDto,
    WorkspaceRequestDto, acknowledgement_from_response, prompt_from_response,
    session_from_response, sessions_from_response,
};
use grok_runtime::{
    GrokRuntime, RedactedDiagnostic, ResolveGrokExecutableError, RuntimeCommand, RuntimeError,
    RuntimeErrorCode, RuntimeEvent, RuntimeState, resolve_grok_executable,
};
use tauri::{Emitter as _, Manager as _};

use crate::presentation::PresentationStore;
use crate::workspace::WorkspaceStore;

struct RuntimeManager {
    runtime: Result<GrokRuntime, RuntimeError>,
    executable_state: ExecutableStateDto,
    executable_source: Option<ExecutableSourceDto>,
    event_clock: Arc<ApplicationEventClock>,
    lifecycle: tokio::sync::Mutex<()>,
    stopped: AtomicBool,
}

impl RuntimeManager {
    fn discover() -> Self {
        let discovery = resolve_grok_executable(None);
        let executable_state = match discovery.as_ref() {
            Ok(_) => ExecutableStateDto::Available,
            Err(ResolveGrokExecutableError::NotFound) => ExecutableStateDto::Missing,
            Err(_) => ExecutableStateDto::Invalid,
        };
        let executable_source = discovery
            .as_ref()
            .ok()
            .map(|executable| executable.source().into());
        let runtime = discovery
            .map(GrokRuntime::from_resolved_executable)
            .map_err(|error| RuntimeError {
                code: RuntimeErrorCode::ExecutableUnavailable,
                diagnostic: RedactedDiagnostic::new(error.to_string()),
                recoverable: true,
            });
        Self {
            runtime,
            executable_state,
            executable_source,
            event_clock: Arc::new(ApplicationEventClock::default()),
            lifecycle: tokio::sync::Mutex::new(()),
            stopped: AtomicBool::new(false),
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

struct WorkspaceManager {
    store: std::sync::Mutex<WorkspaceStore>,
}

impl WorkspaceManager {
    fn new(storage_path: PathBuf) -> Self {
        Self {
            store: std::sync::Mutex::new(WorkspaceStore::open(storage_path)),
        }
    }

    fn select(&self, path: PathBuf) -> Result<WorkspaceDto, ApplicationErrorDto> {
        let canonical = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .select(&path)
            .map_err(ApplicationErrorDto::from)?;
        WorkspaceDto::from_canonical_path(canonical)
    }

    fn recent(&self) -> Result<RecentWorkspaceListDto, ApplicationErrorDto> {
        let recent = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recent()
            .map_err(ApplicationErrorDto::from)?;
        RecentWorkspaceListDto::from_recent(recent)
    }

    fn remove(&self, path: PathBuf) -> Result<RecentWorkspaceListDto, ApplicationErrorDto> {
        let mut store = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        store.remove(&path).map_err(ApplicationErrorDto::from)?;
        RecentWorkspaceListDto::from_recent(store.recent().map_err(ApplicationErrorDto::from)?)
    }

    fn remember_session(&self, workspace: PathBuf, session_id: &str) {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remember_session(&workspace, session_id);
    }

    fn forget_session(&self, session_id: &str) {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forget_session(session_id);
    }
}

struct PresentationManager {
    store: std::sync::Mutex<PresentationStore>,
}

impl PresentationManager {
    fn new(storage_path: PathBuf) -> Self {
        Self {
            store: std::sync::Mutex::new(PresentationStore::open(storage_path)),
        }
    }

    fn get(&self) -> Result<PresentationPreferencesDto, ApplicationErrorDto> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get()
            .map_err(ApplicationErrorDto::from)
    }

    fn set(
        &self,
        preferences: PresentationPreferencesDto,
    ) -> Result<PresentationPreferencesDto, ApplicationErrorDto> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set(preferences)
            .map_err(ApplicationErrorDto::from)
    }
}

async fn stop_contained_runtime(manager: &RuntimeManager) {
    if manager.stopped.swap(true, Ordering::SeqCst) {
        return;
    }
    let _lifecycle = manager.lifecycle.lock().await;
    if let Ok(runtime) = manager.runtime() {
        let _ = runtime.stop().await;
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
            executable_state: manager.executable_state,
            executable_source: manager.executable_source,
            failure: None,
        },
        Err(error) => SetupStatusDto {
            runtime_available: false,
            executable_state: manager.executable_state,
            executable_source: manager.executable_source,
            failure: Some(error.clone().into()),
        },
    }
}

#[tauri::command]
async fn workspace_pick(
    window: tauri::Window,
    manager: tauri::State<'_, WorkspaceManager>,
) -> Result<Option<WorkspaceDto>, ApplicationErrorDto> {
    #[cfg(windows)]
    let selected = rfd::AsyncFileDialog::new()
        .set_parent(&window)
        .set_title("Choose a Grok Build workspace")
        .pick_folder()
        .await
        .map(|handle| handle.path().to_path_buf());

    #[cfg(not(windows))]
    let selected: Option<PathBuf> = {
        let _ = window;
        return Err(ApplicationErrorDto::workspace_picker_unavailable());
    };

    selected.map(|path| manager.select(path)).transpose()
}

#[tauri::command]
fn workspace_validate(
    manager: tauri::State<'_, WorkspaceManager>,
    request: WorkspaceRequestDto,
) -> Result<WorkspaceDto, ApplicationErrorDto> {
    manager.select(request.into_path()?)
}

#[tauri::command]
fn workspace_recent_list(
    manager: tauri::State<'_, WorkspaceManager>,
) -> Result<RecentWorkspaceListDto, ApplicationErrorDto> {
    manager.recent()
}

#[tauri::command]
fn workspace_recent_remove(
    manager: tauri::State<'_, WorkspaceManager>,
    request: WorkspaceRequestDto,
) -> Result<RecentWorkspaceListDto, ApplicationErrorDto> {
    manager.remove(request.into_path()?)
}

#[tauri::command]
fn open_external_url(
    request: OpenExternalUrlRequestDto,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    let url = request.validated_url()?;
    open_validated_external_url(&url)?;
    Ok(AcknowledgementDto::accepted())
}

fn open_validated_external_url(url: &str) -> Result<(), ApplicationErrorDto> {
    let mut command = opener_command();
    command.arg(url);
    command
        .spawn()
        .map(|_| ())
        .map_err(|_| ApplicationErrorDto::external_url_open_failed())
}

fn opener_command() -> std::process::Command {
    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
    }
}

#[tauri::command]
fn runtime_snapshot(
    manager: tauri::State<'_, RuntimeManager>,
) -> Result<RuntimeSnapshotDto, ApplicationErrorDto> {
    manager.snapshot()
}

#[tauri::command]
fn runtime_diagnostics(
    manager: tauri::State<'_, RuntimeManager>,
) -> Result<RuntimeDiagnosticsDto, ApplicationErrorDto> {
    Ok(RuntimeDiagnosticsDto::from_health(
        manager.runtime()?.health(),
    ))
}

#[tauri::command]
async fn workspace_changes(
    request: WorkspaceRequestDto,
) -> Result<application_contract::WorkspaceChangesDto, ApplicationErrorDto> {
    crate::workspace_changes::inspect_workspace_changes(&request.into_path()?).await
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
    workspaces: tauri::State<'_, WorkspaceManager>,
    request: NewSessionRequestDto,
) -> Result<SessionDto, ApplicationErrorDto> {
    let command = request.into_runtime_command()?;
    let workspace = match &command {
        RuntimeCommand::NewSession { workspace } => workspace.clone(),
        _ => return Err(ApplicationErrorDto::unexpected_response()),
    };
    let session = session_from_response(
        manager
            .runtime()?
            .execute(command)
            .await
            .map_err(ApplicationErrorDto::from)?,
    )?;
    workspaces.remember_session(workspace, &session.session_id);
    Ok(session)
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
    workspaces: tauri::State<'_, WorkspaceManager>,
    request: SessionWorkspaceRequestDto,
) -> Result<SessionDto, ApplicationErrorDto> {
    remember_opened_session(manager, workspaces, request.into_load_command()?).await
}

#[tauri::command]
async fn session_resume(
    manager: tauri::State<'_, RuntimeManager>,
    workspaces: tauri::State<'_, WorkspaceManager>,
    request: SessionWorkspaceRequestDto,
) -> Result<SessionDto, ApplicationErrorDto> {
    remember_opened_session(manager, workspaces, request.into_resume_command()?).await
}

async fn remember_opened_session(
    manager: tauri::State<'_, RuntimeManager>,
    workspaces: tauri::State<'_, WorkspaceManager>,
    command: RuntimeCommand,
) -> Result<SessionDto, ApplicationErrorDto> {
    let workspace = match &command {
        RuntimeCommand::LoadSession { workspace, .. }
        | RuntimeCommand::ResumeSession { workspace, .. } => workspace.clone(),
        _ => return Err(ApplicationErrorDto::unexpected_response()),
    };
    let session = session_from_response(
        manager
            .runtime()?
            .execute(command)
            .await
            .map_err(ApplicationErrorDto::from)?,
    )?;
    workspaces.remember_session(workspace, &session.session_id);
    Ok(session)
}

#[tauri::command]
async fn session_close(
    manager: tauri::State<'_, RuntimeManager>,
    workspaces: tauri::State<'_, WorkspaceManager>,
    request: SessionRequestDto,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    let session_id = request.validated_session_id()?;
    let response = manager
        .runtime()?
        .execute(RuntimeCommand::CloseSession {
            session_id: session_id.clone(),
        })
        .await
        .map_err(ApplicationErrorDto::from)?;
    let acknowledgement = acknowledgement_from_response(response)?;
    workspaces.forget_session(&session_id);
    Ok(acknowledgement)
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

#[tauri::command]
fn presentation_get(
    manager: tauri::State<'_, PresentationManager>,
) -> Result<PresentationPreferencesDto, ApplicationErrorDto> {
    manager.get()
}

#[tauri::command]
fn presentation_set(
    manager: tauri::State<'_, PresentationManager>,
    request: PresentationPreferencesDto,
) -> Result<PresentationPreferencesDto, ApplicationErrorDto> {
    manager.set(request)
}

pub fn run() {
    let manager = RuntimeManager::discover();
    let event_source = manager.runtime.as_ref().ok().cloned();
    let event_clock = Arc::clone(&manager.event_clock);

    tauri::Builder::default()
        .manage(manager)
        .setup(move |app| {
            let config_dir = app.path().app_config_dir()?;
            app.manage(WorkspaceManager::new(
                config_dir.join("recent-workspaces.json"),
            ));
            app.manage(PresentationManager::new(
                config_dir.join("presentation.json"),
            ));
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
            workspace_pick,
            workspace_validate,
            workspace_recent_list,
            workspace_recent_remove,
            workspace_changes,
            open_external_url,
            runtime_snapshot,
            runtime_diagnostics,
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
            presentation_get,
            presentation_set,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Grok Build GUI desktop shell")
        .run(|app_handle, event| {
            if matches!(
                event,
                tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
            ) {
                if let Some(manager) = app_handle.try_state::<RuntimeManager>() {
                    tauri::async_runtime::block_on(stop_contained_runtime(&manager));
                }
            }
        });
}
