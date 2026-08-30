use grok_runtime::{
    ElicitationDecision, GrokRuntime, PermissionDecision, RedactedDiagnostic, RuntimeCommand,
    RuntimeError, RuntimeEvent, RuntimeResponse, RuntimeSnapshot,
};
use tauri::Emitter as _;

const RUNTIME_EVENT_NAME: &str = "grok-runtime-event";

struct RuntimeManager {
    runtime: Result<GrokRuntime, RuntimeError>,
}

fn emit_runtime_event(app: &tauri::AppHandle, event: RuntimeEvent) {
    let _ = app.emit(RUNTIME_EVENT_NAME, event);
}

impl RuntimeManager {
    fn discover() -> Self {
        Self {
            runtime: GrokRuntime::discover(None),
        }
    }

    fn runtime(&self) -> Result<&GrokRuntime, RuntimeError> {
        self.runtime.as_ref().map_err(Clone::clone)
    }
}

#[tauri::command]
fn runtime_snapshot(
    manager: tauri::State<'_, RuntimeManager>,
) -> Result<RuntimeSnapshot, RuntimeError> {
    manager.runtime().map(GrokRuntime::snapshot)
}

#[tauri::command]
async fn runtime_start(
    manager: tauri::State<'_, RuntimeManager>,
) -> Result<RuntimeSnapshot, RuntimeError> {
    manager.runtime()?.start().await
}

#[tauri::command]
async fn runtime_stop(manager: tauri::State<'_, RuntimeManager>) -> Result<(), RuntimeError> {
    manager.runtime()?.stop().await
}

#[tauri::command]
async fn runtime_restart(
    manager: tauri::State<'_, RuntimeManager>,
) -> Result<RuntimeSnapshot, RuntimeError> {
    manager.runtime()?.restart().await
}

#[tauri::command]
async fn runtime_execute(
    manager: tauri::State<'_, RuntimeManager>,
    command: RuntimeCommand,
) -> Result<RuntimeResponse, RuntimeError> {
    manager.runtime()?.execute(command).await
}

#[tauri::command]
async fn runtime_respond_permission(
    manager: tauri::State<'_, RuntimeManager>,
    interaction_id: String,
    decision: PermissionDecision,
) -> Result<(), RuntimeError> {
    manager
        .runtime()?
        .respond_permission(&interaction_id, decision)
        .await
}

#[tauri::command]
async fn runtime_respond_elicitation(
    manager: tauri::State<'_, RuntimeManager>,
    interaction_id: String,
    decision: ElicitationDecision,
) -> Result<(), RuntimeError> {
    manager
        .runtime()?
        .respond_elicitation(&interaction_id, decision)
        .await
}

pub fn run() {
    let manager = RuntimeManager::discover();
    let event_source = manager.runtime.as_ref().ok().cloned();

    tauri::Builder::default()
        .manage(RuntimeManager {
            runtime: manager.runtime,
        })
        .setup(move |app| {
            if let Some(runtime) = event_source {
                let mut events = runtime.subscribe();
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        match events.recv().await {
                            Ok(event) => emit_runtime_event(&app_handle, event),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                emit_runtime_event(
                                    &app_handle,
                                    RuntimeEvent::RuntimeFailed {
                                        diagnostic: RedactedDiagnostic::new(
                                            "runtime event delivery fell behind; reconnect required",
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
            runtime_snapshot,
            runtime_start,
            runtime_stop,
            runtime_restart,
            runtime_execute,
            runtime_respond_permission,
            runtime_respond_elicitation,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Grok Build GUI desktop shell");
}
