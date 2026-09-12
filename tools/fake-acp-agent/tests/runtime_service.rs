use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

use grok_runtime::{
    ActivityStatus, ElicitationDecision, ElicitationValue, GrokRuntime, PermissionDecision,
    PermissionKind, RuntimeCommand, RuntimeConfigKind, RuntimeConfigValue, RuntimeEvent,
    RuntimeExtensionUpdate, RuntimePromptStopReason, RuntimeResponse, RuntimeState,
    RuntimeTestTarget,
};

#[tokio::test]
async fn long_lived_runtime_lifecycle_is_idempotent_and_restartable() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    let mut events = runtime.subscribe();

    let first = runtime.start().await.expect("runtime should start");
    assert_eq!(first.state, RuntimeState::Ready);
    assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
    assert_eq!(
        runtime
            .start()
            .await
            .expect("repeated start should be idempotent"),
        first
    );

    assert_state_sequence(
        &mut events,
        &[
            RuntimeState::Connecting,
            RuntimeState::Authenticating,
            RuntimeState::Ready,
        ],
    )
    .await;
    assert!(events.try_recv().is_err(), "repeated start emitted events");

    runtime.stop().await.expect("runtime should stop");
    runtime
        .stop()
        .await
        .expect("repeated stop should be idempotent");
    assert_eq!(runtime.snapshot().state, RuntimeState::Disconnected);
    assert_state_sequence(&mut events, &[RuntimeState::Disconnected]).await;
    assert!(events.try_recv().is_err(), "repeated stop emitted events");

    let restarted = runtime.restart().await.expect("runtime should restart");
    assert_eq!(restarted.state, RuntimeState::Ready);
    assert_state_sequence(
        &mut events,
        &[
            RuntimeState::Connecting,
            RuntimeState::Authenticating,
            RuntimeState::Ready,
        ],
    )
    .await;

    runtime.stop().await.expect("restarted runtime should stop");
}

#[tokio::test]
async fn dropping_the_last_runtime_owner_terminates_its_process_tree() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory");
    let sentinel_directory = temporary_directory.path().join("runtime-drop-sentinel");
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args([
                "descendant",
                "--sentinel-dir",
                sentinel_directory
                    .to_str()
                    .expect("temporary path should be Unicode"),
            ])
            .auth_method("fixture_auth"),
    );

    runtime.start().await.expect("runtime should start");
    wait_for_heartbeat(&sentinel_directory.join("sentinel.heartbeat"), 0).await;
    drop(runtime);

    wait_for_stable_heartbeat(&sentinel_directory.join("sentinel.heartbeat")).await;
}

#[tokio::test]
async fn stop_is_bounded_when_the_child_lingers_after_protocol_eof() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory");
    let sentinel_directory = temporary_directory.path().join("bounded-stop-sentinel");
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args([
                "descendant",
                "--sentinel-dir",
                sentinel_directory
                    .to_str()
                    .expect("temporary path should be Unicode"),
                "--linger-after-eof",
            ])
            .auth_method("fixture_auth"),
    );

    runtime.start().await.expect("runtime should start");
    wait_for_heartbeat(&sentinel_directory.join("sentinel.heartbeat"), 0).await;
    tokio::time::timeout(Duration::from_secs(3), runtime.stop())
        .await
        .expect("runtime stop must remain bounded")
        .expect("runtime should stop");
    wait_for_stable_heartbeat(&sentinel_directory.join("sentinel.heartbeat")).await;
}

#[tokio::test]
async fn stop_during_hung_initialize_does_not_wait_for_ready() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args(["lifecycle", "--lifecycle-fault", "hang-initialize"])
            .auth_method("fixture_auth")
            .request_timeout(Duration::from_secs(30)),
    );
    let mut events = runtime.subscribe();
    let starting = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.start().await })
    };
    wait_for_runtime_state(&mut events, RuntimeState::Connecting).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    tokio::time::timeout(Duration::from_secs(3), runtime.stop())
        .await
        .expect("stop during hung initialize must remain bounded")
        .expect("runtime should stop");
    let start_result = tokio::time::timeout(Duration::from_secs(3), starting)
        .await
        .expect("start should unwind after stop")
        .expect("start task should join");
    assert!(
        start_result.is_err(),
        "stop must interrupt initialize instead of letting it finish"
    );
    assert!(!runtime.health().worker_running);
}

#[tokio::test]
async fn a_crashed_runtime_reports_failure_and_can_restart_cleanly() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory");
    let state_file = temporary_directory.path().join("persisted-session.state");
    let workspace = temporary_directory.path().join("workspace");
    fs::create_dir(&workspace).expect("temporary workspace");
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args([
                "crash-after-new",
                "--state-file",
                state_file
                    .to_str()
                    .expect("temporary path should be Unicode"),
            ])
            .auth_method("fixture_auth"),
    );
    let mut events = runtime.subscribe();

    runtime.start().await.expect("runtime should start");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.clone(),
        })
        .await
        .expect("session response should arrive before the fixture crashes")
    else {
        panic!("expected a session response");
    };
    wait_for_runtime_state(&mut events, RuntimeState::Failed).await;
    assert_eq!(runtime.snapshot().state, RuntimeState::Failed);

    let restarted = runtime.restart().await.expect("runtime should restart");
    assert_eq!(restarted.state, RuntimeState::Ready);
    let RuntimeResponse::Sessions(sessions) = runtime
        .execute(RuntimeCommand::ListSessions {
            workspace: Some(workspace.clone()),
            cursor: None,
        })
        .await
        .expect("restarted runtime should list persisted sessions")
    else {
        panic!("expected a session page");
    };
    assert_eq!(sessions.sessions[0].session_id, session.session_id);
    let RuntimeResponse::Session(resumed) = runtime
        .execute(RuntimeCommand::ResumeSession {
            session_id: session.session_id.clone(),
            workspace: workspace.clone(),
        })
        .await
        .expect("restarted runtime should resume the persisted session")
    else {
        panic!("expected a resumed session");
    };
    assert_eq!(resumed.session_id, session.session_id);
    runtime.stop().await.expect("restarted runtime should stop");
}

#[tokio::test]
async fn crash_loops_keep_a_single_runtime_and_remain_recoverable() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).args(["crash"]),
    );

    for expected in 1_u32..=3 {
        let error = runtime
            .start()
            .await
            .expect_err("crash fixture should fail start");
        assert!(error.recoverable);
        let health = runtime.health();
        assert!(
            !health.worker_running,
            "a failed start must not leave a worker"
        );
        assert_eq!(health.consecutive_failures, expected);
        assert_eq!(health.state, RuntimeState::Failed);
        assert!(health.last_error.is_some());
    }

    runtime
        .stop()
        .await
        .expect("stop after a crash loop should be idempotent");
    assert!(!runtime.health().worker_running);
}

#[tokio::test]
async fn concurrent_start_reuses_one_worker() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    let first = runtime.start();
    let second = runtime.start();
    let (first, second) = tokio::join!(first, second);
    let first = first.expect("first start should succeed");
    let second = second.expect("second start should reuse the worker");
    assert_eq!(first.state, RuntimeState::Ready);
    assert_eq!(second.state, RuntimeState::Ready);
    assert!(runtime.health().worker_running);
    runtime.stop().await.expect("runtime should stop");
    assert!(!runtime.health().worker_running);
}

#[tokio::test]
async fn session_lifecycle_is_exposed_without_acp_method_names() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");

    let created = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created");
    let RuntimeResponse::Session(session) = created else {
        panic!("expected a session response");
    };
    assert_eq!(session.session_id, "session-001");

    let listed = runtime
        .execute(RuntimeCommand::ListSessions {
            workspace: Some(workspace.path().to_path_buf()),
            cursor: None,
        })
        .await
        .expect("sessions should be listed");
    let RuntimeResponse::Sessions(page) = listed else {
        panic!("expected a session page");
    };
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].session_id, "session-001");
    assert_eq!(
        page.sessions[0].title.as_deref(),
        Some("Sanitized fixture session")
    );
    assert!(page.next_cursor.is_none());

    let mut events = runtime.subscribe();
    for command in [
        RuntimeCommand::ResumeSession {
            session_id: session.session_id.clone(),
            workspace: workspace.path().to_path_buf(),
        },
        RuntimeCommand::LoadSession {
            session_id: session.session_id.clone(),
            workspace: workspace.path().to_path_buf(),
        },
    ] {
        let RuntimeResponse::Session(recovered) = runtime
            .execute(command)
            .await
            .expect("session recovery should succeed")
        else {
            panic!("expected a recovered session");
        };
        assert_eq!(recovered.session_id, session.session_id);
        assert!(matches!(
            events.recv().await.expect("activation event"),
            RuntimeEvent::SessionActivated { ref session_id }
                if session_id == &session.session_id
        ));
        loop {
            if matches!(
                events.recv().await.expect("ready event"),
                RuntimeEvent::SessionStateChanged {
                    ref session_id,
                    state: grok_runtime::SessionState::Ready,
                } if session_id == &session.session_id
            ) {
                break;
            }
        }
    }

    assert_eq!(
        runtime
            .execute(RuntimeCommand::CloseSession {
                session_id: session.session_id,
            })
            .await
            .expect("session should close"),
        RuntimeResponse::Acknowledged
    );
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn sessions_cannot_be_reused_across_workspaces() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("selected workspace");
    let other = tempfile::tempdir().expect("other workspace");

    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };

    let RuntimeResponse::Sessions(foreign) = runtime
        .execute(RuntimeCommand::ListSessions {
            workspace: Some(other.path().to_path_buf()),
            cursor: None,
        })
        .await
        .expect("foreign workspace list should succeed")
    else {
        panic!("expected a session page");
    };
    assert!(
        foreign.sessions.is_empty(),
        "a workspace must not inherit another workspace's sessions"
    );

    let resume_error = runtime
        .execute(RuntimeCommand::ResumeSession {
            session_id: session.session_id.clone(),
            workspace: other.path().to_path_buf(),
        })
        .await
        .expect_err("resume must stay inside the session workspace");
    assert_eq!(
        resume_error.code,
        grok_runtime::RuntimeErrorCode::InvalidWorkspace
    );

    let load_error = runtime
        .execute(RuntimeCommand::LoadSession {
            session_id: session.session_id.clone(),
            workspace: other.path().to_path_buf(),
        })
        .await
        .expect_err("load must stay inside the session workspace");
    assert_eq!(
        load_error.code,
        grok_runtime::RuntimeErrorCode::InvalidWorkspace
    );

    let RuntimeResponse::Sessions(owned) = runtime
        .execute(RuntimeCommand::ListSessions {
            workspace: Some(workspace.path().to_path_buf()),
            cursor: None,
        })
        .await
        .expect("owned workspace list should succeed")
    else {
        panic!("expected a session page");
    };
    assert_eq!(owned.sessions.len(), 1);
    assert_eq!(owned.sessions[0].session_id, session.session_id);

    runtime
        .execute(RuntimeCommand::ResumeSession {
            session_id: session.session_id,
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("resume inside the original workspace should succeed");
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn session_list_and_open_failures_are_reported() {
    let list_runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args(["lifecycle", "--lifecycle-fault", "list-error"])
            .auth_method("fixture_auth"),
    );
    list_runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    list_runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created");
    let list_error = list_runtime
        .execute(RuntimeCommand::ListSessions {
            workspace: Some(workspace.path().to_path_buf()),
            cursor: None,
        })
        .await
        .expect_err("fixture list should fail");
    assert_eq!(
        list_error.code,
        grok_runtime::RuntimeErrorCode::ProtocolRequestFailed
    );
    list_runtime.stop().await.expect("runtime should stop");

    for (fault, command) in [
        (
            "load-error",
            RuntimeCommand::LoadSession {
                session_id: "session-001".to_owned(),
                workspace: workspace.path().to_path_buf(),
            },
        ),
        (
            "resume-error",
            RuntimeCommand::ResumeSession {
                session_id: "session-001".to_owned(),
                workspace: workspace.path().to_path_buf(),
            },
        ),
    ] {
        let runtime = GrokRuntime::for_test(
            RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
                .args(["lifecycle", "--lifecycle-fault", fault])
                .auth_method("fixture_auth"),
        );
        runtime.start().await.expect("runtime should start");
        runtime
            .execute(RuntimeCommand::NewSession {
                workspace: workspace.path().to_path_buf(),
            })
            .await
            .expect("session should be created");
        let error = runtime
            .execute(command)
            .await
            .expect_err("fixture open should fail");
        assert_eq!(
            error.code,
            grok_runtime::RuntimeErrorCode::ProtocolRequestFailed
        );
        runtime.stop().await.expect("runtime should stop");
    }
}

#[tokio::test]
async fn prompt_streams_domain_events_and_accepts_scoped_user_decisions() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    let mut events = runtime.subscribe();

    let prompt_runtime = runtime.clone();
    let session_id = session.session_id.clone();
    let prompt = tokio::spawn(async move {
        prompt_runtime
            .execute(RuntimeCommand::Prompt {
                session_id,
                text: "Exercise the deterministic fixture.".to_owned(),
            })
            .await
    });

    let mut saw_thought = false;
    let mut saw_message = false;
    let mut saw_plan = false;
    let mut saw_completed_tool = false;
    let mut handled_permission = false;
    let mut handled_elicitation = false;

    while !(saw_thought
        && saw_message
        && saw_plan
        && saw_completed_tool
        && handled_permission
        && handled_elicitation)
    {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("prompt event timed out")
            .expect("runtime event channel closed");
        match event {
            RuntimeEvent::ThoughtChunkReceived { text, .. } => {
                saw_thought |= text.contains("deterministic fixture");
            }
            RuntimeEvent::MessageChunkReceived { text, .. } => {
                saw_message |= text.contains("inspect the sanitized fixture");
            }
            RuntimeEvent::PlanChanged { entries, .. } => {
                saw_plan |= entries.len() == 2;
            }
            RuntimeEvent::ToolCallChanged { status, .. } => {
                saw_completed_tool |= status == ActivityStatus::Completed;
            }
            RuntimeEvent::PermissionRequested {
                interaction_id,
                kind,
                ..
            } => {
                assert_eq!(
                    kind,
                    PermissionKind::Command {
                        command: "fixture-tool --check fixture.txt".to_owned(),
                        working_directory: Some(fixture_permission_workspace()),
                        affected_paths: vec![fixture_permission_workspace().join("fixture.txt")],
                    }
                );
                runtime
                    .respond_permission(&interaction_id, PermissionDecision::AllowOnce)
                    .await
                    .expect("permission response should be accepted");
                handled_permission = true;
            }
            RuntimeEvent::ElicitationRequested { interaction_id, .. } => {
                runtime
                    .respond_elicitation(
                        &interaction_id,
                        ElicitationDecision::Accept {
                            content: BTreeMap::from([(
                                "label".to_owned(),
                                ElicitationValue::String("fixture-ok".to_owned()),
                            )]),
                        },
                    )
                    .await
                    .expect("elicitation response should be accepted");
                handled_elicitation = true;
            }
            _ => {}
        }
    }

    assert_eq!(
        prompt
            .await
            .expect("prompt task should join")
            .expect("prompt should succeed"),
        RuntimeResponse::PromptCompleted {
            stop_reason: RuntimePromptStopReason::EndTurn,
        }
    );
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn advertised_session_controls_can_be_changed_and_active_prompts_cancelled() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    let mut events = runtime.subscribe();
    let snapshot = runtime.start().await.expect("runtime should start");
    assert_eq!(
        snapshot
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.models.as_ref())
            .map(|models| models.current_model_id.as_str()),
        Some("fixture-model")
    );
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    assert_eq!(
        session
            .controls
            .modes
            .as_ref()
            .map(|modes| modes.current_mode_id.as_str()),
        Some("default")
    );
    assert_eq!(session.controls.config_options.len(), 1);

    for command in [
        RuntimeCommand::SetSessionMode {
            session_id: session.session_id.clone(),
            mode_id: "plan".to_owned(),
        },
        RuntimeCommand::SetSessionModel {
            session_id: session.session_id.clone(),
            model_id: "fixture-model".to_owned(),
            reasoning_effort: Some("high".to_owned()),
        },
        RuntimeCommand::SetSessionConfig {
            session_id: session.session_id.clone(),
            config_id: "fixture-toggle".to_owned(),
            value: RuntimeConfigValue::Boolean(true),
        },
    ] {
        assert_eq!(
            runtime
                .execute(command)
                .await
                .expect("advertised control should update"),
            RuntimeResponse::Acknowledged
        );
    }

    let mut saw_mode = false;
    let mut saw_config = false;
    let mut saw_models = false;
    let mut saw_session_extension = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(saw_mode && saw_config && saw_models && saw_session_extension) {
            match events
                .recv()
                .await
                .expect("runtime event stream should stay open")
            {
                RuntimeEvent::SessionModeChanged {
                    current_mode_id, ..
                } if current_mode_id == "plan" => saw_mode = true,
                RuntimeEvent::SessionConfigOptionsChanged { config_options, .. }
                    if matches!(
                        config_options.as_slice(),
                        [option]
                            if matches!(
                                &option.kind,
                                RuntimeConfigKind::Boolean { current_value: true }
                            )
                    ) =>
                {
                    saw_config = true;
                }
                RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Models(models),
                } if models.current_model_id == "fixture-model"
                    && models.available_models.len() == 1 =>
                {
                    saw_models = true;
                }
                RuntimeEvent::RuntimeExtensionChanged {
                    update: RuntimeExtensionUpdate::Session(_),
                } => saw_session_extension = true,
                _ => {}
            }
        }
    })
    .await
    .expect("timed out waiting for detailed control updates");

    assert_eq!(
        runtime
            .snapshot()
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.models.as_ref())
            .map(|models| models.current_model_id.as_str()),
        Some("fixture-model")
    );

    complete_first_fixture_prompt(&runtime, &session.session_id).await;
    let prompt_runtime = runtime.clone();
    let session_id = session.session_id.clone();
    let pending_prompt = tokio::spawn(async move {
        prompt_runtime
            .execute(RuntimeCommand::Prompt {
                session_id,
                text: "Wait until cancelled.".to_owned(),
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        runtime
            .execute(RuntimeCommand::Cancel {
                session_id: session.session_id,
            })
            .await
            .expect("cancel should be delivered"),
        RuntimeResponse::Acknowledged
    );
    assert_eq!(
        pending_prompt
            .await
            .expect("cancelled prompt task should join")
            .expect("cancelled prompt should return a semantic result"),
        RuntimeResponse::PromptCompleted {
            stop_reason: RuntimePromptStopReason::Cancelled,
        }
    );
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn cancellation_denies_a_pending_interaction_and_expires_its_gui_id() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    let mut events = runtime.subscribe();
    let prompt_runtime = runtime.clone();
    let session_id = session.session_id.clone();
    let prompt = tokio::spawn(async move {
        prompt_runtime
            .execute(RuntimeCommand::Prompt {
                session_id,
                text: "Cancel at the permission boundary.".to_owned(),
            })
            .await
    });
    let interaction_id = loop {
        if let RuntimeEvent::PermissionRequested { interaction_id, .. } = events
            .recv()
            .await
            .expect("runtime event channel should remain open")
        {
            break interaction_id;
        }
    };

    runtime
        .execute(RuntimeCommand::Cancel {
            session_id: session.session_id.clone(),
        })
        .await
        .expect("cancel should be delivered");
    loop {
        if matches!(
            events
                .recv()
                .await
                .expect("runtime event channel should remain open"),
            RuntimeEvent::InteractionsCleared { session_id: Some(ref cleared) }
                if cleared == &session.session_id
        ) {
            break;
        }
    }
    assert_eq!(
        prompt
            .await
            .expect("prompt task should join")
            .expect("fixture should return a cancellation result"),
        RuntimeResponse::PromptCompleted {
            stop_reason: RuntimePromptStopReason::Cancelled,
        }
    );
    assert!(
        runtime
            .respond_permission(&interaction_id, PermissionDecision::AllowOnce)
            .await
            .is_err(),
        "cancelled GUI interaction IDs must expire"
    );
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn closing_a_session_cancels_pending_interactions_and_expires_their_gui_ids() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    let mut events = runtime.subscribe();
    let prompt_runtime = runtime.clone();
    let session_id = session.session_id.clone();
    let prompt = tokio::spawn(async move {
        prompt_runtime
            .execute(RuntimeCommand::Prompt {
                session_id,
                text: "Close at the permission boundary.".to_owned(),
            })
            .await
    });
    let interaction_id = loop {
        if let RuntimeEvent::PermissionRequested { interaction_id, .. } =
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("permission event timed out")
                .expect("runtime event channel should remain open")
        {
            break interaction_id;
        }
    };

    assert_eq!(
        runtime
            .execute(RuntimeCommand::CloseSession {
                session_id: session.session_id.clone(),
            })
            .await
            .expect("session should close"),
        RuntimeResponse::Acknowledged
    );

    let mut saw_clear = false;
    let mut saw_closed = false;
    while !(saw_clear && saw_closed) {
        match tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("close event timed out")
            .expect("runtime event channel should remain open")
        {
            RuntimeEvent::InteractionsCleared {
                session_id: Some(ref cleared),
            } if cleared == &session.session_id => saw_clear = true,
            RuntimeEvent::SessionStateChanged {
                ref session_id,
                state: grok_runtime::SessionState::Closed,
            } if session_id == &session.session_id => saw_closed = true,
            RuntimeEvent::SessionStateChanged {
                ref session_id,
                state: grok_runtime::SessionState::Ready,
            } if session_id == &session.session_id => {
                panic!("session must not return to ready while close is in flight");
            }
            _ => {}
        }
    }
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), prompt)
            .await
            .expect("prompt cancellation timed out")
            .expect("prompt task should join")
            .expect("fixture should return a cancellation result"),
        RuntimeResponse::PromptCompleted {
            stop_reason: RuntimePromptStopReason::Cancelled,
        }
    );
    while let Ok(event) = events.try_recv() {
        assert!(
            !matches!(
                event,
                RuntimeEvent::SessionStateChanged {
                    ref session_id,
                    state: grok_runtime::SessionState::Ready,
                } if session_id == &session.session_id
            ),
            "prompt completion after close must not resurrect session readiness"
        );
    }
    assert!(
        runtime
            .respond_permission(&interaction_id, PermissionDecision::AllowOnce)
            .await
            .is_err(),
        "closed-session GUI interaction IDs must expire"
    );
    let error = runtime
        .execute(RuntimeCommand::Prompt {
            session_id: session.session_id.clone(),
            text: "A closed session must not restart.".to_owned(),
        })
        .await
        .expect_err("closed sessions require explicit load or resume");
    assert_eq!(error.code, grok_runtime::RuntimeErrorCode::InvalidRequest);
    let error = runtime
        .execute(RuntimeCommand::CloseSession {
            session_id: session.session_id,
        })
        .await
        .expect_err("an unavailable session must not be closed again");
    assert_eq!(error.code, grok_runtime::RuntimeErrorCode::InvalidRequest);
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn rejected_close_restores_the_open_session_and_its_interaction_gate() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args(["lifecycle", "--lifecycle-fault", "close-error"])
            .auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    let mut events = runtime.subscribe();

    runtime
        .execute(RuntimeCommand::CloseSession {
            session_id: session.session_id.clone(),
        })
        .await
        .expect_err("fixture close should be rejected");
    loop {
        if matches!(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("close recovery event timed out")
                .expect("runtime event channel should remain open"),
            RuntimeEvent::SessionStateChanged {
                ref session_id,
                state: grok_runtime::SessionState::Ready,
            } if session_id == &session.session_id
        ) {
            break;
        }
    }

    complete_first_fixture_prompt(&runtime, &session.session_id).await;
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn timed_out_close_stays_fail_closed_until_explicit_reactivation() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args(["lifecycle", "--lifecycle-fault", "hang-close"])
            .auth_method("fixture_auth")
            .request_timeout(Duration::from_secs(2)),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    let mut events = runtime.subscribe();

    let close_runtime = runtime.clone();
    let close_session_id = session.session_id.clone();
    let close = tokio::spawn(async move {
        close_runtime
            .execute(RuntimeCommand::CloseSession {
                session_id: close_session_id,
            })
            .await
    });
    loop {
        if matches!(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("close-start event timed out")
                .expect("runtime event channel should remain open"),
            RuntimeEvent::SessionStateChanged {
                ref session_id,
                state: grok_runtime::SessionState::Cancelling,
            } if session_id == &session.session_id
        ) {
            break;
        }
    }
    let error = runtime
        .execute(RuntimeCommand::Prompt {
            session_id: session.session_id.clone(),
            text: "A closing session must stay blocked.".to_owned(),
        })
        .await
        .expect_err("close-in-progress must reject a concurrent prompt");
    assert_eq!(error.code, grok_runtime::RuntimeErrorCode::InvalidRequest);
    let error = close
        .await
        .expect("close task should join")
        .expect_err("fixture close should time out");
    assert_eq!(error.code, grok_runtime::RuntimeErrorCode::RequestTimedOut);
    loop {
        if matches!(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("ambiguous close event timed out")
                .expect("runtime event channel should remain open"),
            RuntimeEvent::SessionStateChanged {
                ref session_id,
                state: grok_runtime::SessionState::Failed,
            } if session_id == &session.session_id
        ) {
            break;
        }
    }
    let error = runtime
        .execute(RuntimeCommand::Prompt {
            session_id: session.session_id.clone(),
            text: "An uncertain session must stay blocked.".to_owned(),
        })
        .await
        .expect_err("ambiguous close must remain fail-closed");
    assert_eq!(error.code, grok_runtime::RuntimeErrorCode::InvalidRequest);
    let RuntimeResponse::Session(reactivated) = runtime
        .execute(RuntimeCommand::LoadSession {
            session_id: session.session_id.clone(),
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("explicit load should reactivate the session")
    else {
        panic!("expected a reactivated session response");
    };
    assert_eq!(reactivated.session_id, session.session_id);
    complete_first_fixture_prompt(&runtime, &reactivated.session_id).await;
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn cancellation_is_idempotent_and_settles_cancelled_without_ready() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    complete_first_fixture_prompt(&runtime, &session.session_id).await;
    let mut events = runtime.subscribe();

    let prompt_runtime = runtime.clone();
    let session_id = session.session_id.clone();
    let pending_prompt = tokio::spawn(async move {
        prompt_runtime
            .execute(RuntimeCommand::Prompt {
                session_id,
                text: "Wait until cancelled.".to_owned(),
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    for _ in 0..2 {
        assert_eq!(
            runtime
                .execute(RuntimeCommand::Cancel {
                    session_id: session.session_id.clone(),
                })
                .await
                .expect("cancel should be acknowledged"),
            RuntimeResponse::Acknowledged
        );
    }
    assert_eq!(
        pending_prompt
            .await
            .expect("cancelled prompt task should join")
            .expect("cancelled prompt should return a semantic result"),
        RuntimeResponse::PromptCompleted {
            stop_reason: RuntimePromptStopReason::Cancelled,
        }
    );

    let mut saw_cancelling = false;
    let mut saw_cancelled = false;
    while let Ok(event) = events.try_recv() {
        match event {
            RuntimeEvent::SessionStateChanged {
                ref session_id,
                state: grok_runtime::SessionState::Cancelling,
            } if session_id == &session.session_id => saw_cancelling = true,
            RuntimeEvent::SessionStateChanged {
                ref session_id,
                state: grok_runtime::SessionState::Cancelled,
            } if session_id == &session.session_id => saw_cancelled = true,
            RuntimeEvent::SessionStateChanged {
                ref session_id,
                state: grok_runtime::SessionState::Ready,
            } if session_id == &session.session_id && saw_cancelling => {
                panic!("cancelled turns must not return to ready");
            }
            RuntimeEvent::SessionStateChanged {
                ref session_id,
                state: grok_runtime::SessionState::Working,
            } if session_id == &session.session_id && saw_cancelling => {
                panic!("late working must not resurrect a cancelled turn");
            }
            _ => {}
        }
    }
    assert!(saw_cancelling, "cancel should emit cancelling");
    assert!(saw_cancelled, "settled cancel should emit cancelled");
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn idle_cancellation_is_a_no_op_for_the_active_turn() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    assert_eq!(
        runtime
            .execute(RuntimeCommand::Cancel {
                session_id: session.session_id.clone(),
            })
            .await
            .expect("idle cancel should be acknowledged"),
        RuntimeResponse::Acknowledged
    );
    complete_first_fixture_prompt(&runtime, &session.session_id).await;
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn advertised_plan_review_replaces_the_saved_plan() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent")).auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    complete_first_fixture_prompt(&runtime, &session.session_id).await;
    let mut events = runtime.subscribe();
    let response = runtime
        .execute(RuntimeCommand::Prompt {
            session_id: session.session_id.clone(),
            text: "/approve_plan".to_owned(),
        })
        .await
        .expect("advertised plan approval should run");
    assert_eq!(
        response,
        RuntimeResponse::PromptCompleted {
            stop_reason: RuntimePromptStopReason::EndTurn,
        }
    );
    let mut saw_approved_plan = false;
    let mut saw_completed = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(saw_approved_plan && saw_completed) {
            match events
                .recv()
                .await
                .expect("runtime event stream should stay open")
            {
                RuntimeEvent::PlanChanged { entries, .. }
                    if entries.first().is_some_and(|entry| {
                        entry.status == grok_runtime::PlanEntryStatus::Completed
                    }) =>
                {
                    saw_approved_plan = true;
                }
                RuntimeEvent::SessionStateChanged {
                    ref session_id,
                    state: grok_runtime::SessionState::Completed,
                } if session_id == &session.session_id => saw_completed = true,
                _ => {}
            }
        }
    })
    .await
    .expect("timed out waiting for advertised plan review");
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn process_loss_during_a_hung_turn_fails_the_prompt() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args(["lifecycle", "--lifecycle-fault", "crash-prompt"])
            .auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    let prompt_runtime = runtime.clone();
    let session_id = session.session_id.clone();
    let pending_prompt = tokio::spawn(async move {
        prompt_runtime
            .execute(RuntimeCommand::Prompt {
                session_id,
                text: "This turn crashes the agent process.".to_owned(),
            })
            .await
    });
    let error = pending_prompt
        .await
        .expect("hung prompt task should join")
        .expect_err("process loss must fail the hung prompt");
    assert_ne!(error.code, grok_runtime::RuntimeErrorCode::InvalidRequest);
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn cancel_during_hung_turn_settles_cancelled() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args(["lifecycle", "--lifecycle-fault", "hang-prompt"])
            .auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    let prompt_runtime = runtime.clone();
    let session_id = session.session_id.clone();
    let pending_prompt = tokio::spawn(async move {
        prompt_runtime
            .execute(RuntimeCommand::Prompt {
                session_id,
                text: "This turn hangs until cancelled.".to_owned(),
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        runtime
            .execute(RuntimeCommand::Cancel {
                session_id: session.session_id,
            })
            .await
            .expect("cancel should be acknowledged"),
        RuntimeResponse::Acknowledged
    );
    let response = pending_prompt
        .await
        .expect("hung prompt task should join")
        .expect("cancelled prompt should complete");
    assert_eq!(
        response,
        RuntimeResponse::PromptCompleted {
            stop_reason: RuntimePromptStopReason::Cancelled,
        }
    );
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn close_during_hung_turn_settles_cancelled() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args(["lifecycle", "--lifecycle-fault", "hang-prompt"])
            .auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let RuntimeResponse::Session(session) = runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created")
    else {
        panic!("expected a session response");
    };
    let prompt_runtime = runtime.clone();
    let session_id = session.session_id.clone();
    let pending_prompt = tokio::spawn(async move {
        prompt_runtime
            .execute(RuntimeCommand::Prompt {
                session_id,
                text: "This turn hangs until the session is closed.".to_owned(),
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        runtime
            .execute(RuntimeCommand::CloseSession {
                session_id: session.session_id,
            })
            .await
            .expect("close should be acknowledged"),
        RuntimeResponse::Acknowledged
    );
    let response = pending_prompt
        .await
        .expect("hung prompt task should join")
        .expect("closed prompt should complete");
    assert_eq!(
        response,
        RuntimeResponse::PromptCompleted {
            stop_reason: RuntimePromptStopReason::Cancelled,
        }
    );
    runtime.stop().await.expect("runtime should stop");
}

#[tokio::test]
async fn session_list_pagination_returns_the_next_workspace_page() {
    let runtime = GrokRuntime::for_test(
        RuntimeTestTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args(["lifecycle", "--paginate-sessions"])
            .auth_method("fixture_auth"),
    );
    runtime.start().await.expect("runtime should start");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    runtime
        .execute(RuntimeCommand::NewSession {
            workspace: workspace.path().to_path_buf(),
        })
        .await
        .expect("session should be created");

    let RuntimeResponse::Sessions(page_one) = runtime
        .execute(RuntimeCommand::ListSessions {
            workspace: Some(workspace.path().to_path_buf()),
            cursor: None,
        })
        .await
        .expect("first page should list")
    else {
        panic!("expected a session page");
    };
    assert_eq!(page_one.sessions.len(), 1);
    assert_eq!(page_one.sessions[0].session_id, "session-001");
    assert_eq!(page_one.next_cursor.as_deref(), Some("fixture-page-2"));

    let RuntimeResponse::Sessions(page_two) = runtime
        .execute(RuntimeCommand::ListSessions {
            workspace: Some(workspace.path().to_path_buf()),
            cursor: page_one.next_cursor,
        })
        .await
        .expect("second page should list")
    else {
        panic!("expected a session page");
    };
    assert_eq!(page_two.sessions.len(), 1);
    assert_eq!(page_two.sessions[0].session_id, "session-002");
    assert!(page_two.next_cursor.is_none());
    runtime.stop().await.expect("runtime should stop");
}

async fn complete_first_fixture_prompt(runtime: &GrokRuntime, session_id: &str) {
    let mut events = runtime.subscribe();
    let prompt_runtime = runtime.clone();
    let session_id = session_id.to_owned();
    let prompt = tokio::spawn(async move {
        prompt_runtime
            .execute(RuntimeCommand::Prompt {
                session_id,
                text: "Complete the first fixture prompt.".to_owned(),
            })
            .await
    });
    let mut permission = false;
    let mut elicitation = false;
    while !(permission && elicitation) {
        match tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("interaction event timed out")
            .expect("runtime event channel closed")
        {
            RuntimeEvent::PermissionRequested { interaction_id, .. } => {
                runtime
                    .respond_permission(&interaction_id, PermissionDecision::AllowOnce)
                    .await
                    .expect("permission response");
                permission = true;
            }
            RuntimeEvent::ElicitationRequested { interaction_id, .. } => {
                runtime
                    .respond_elicitation(
                        &interaction_id,
                        ElicitationDecision::Accept {
                            content: BTreeMap::from([(
                                "label".to_owned(),
                                ElicitationValue::String("fixture-ok".to_owned()),
                            )]),
                        },
                    )
                    .await
                    .expect("elicitation response");
                elicitation = true;
            }
            _ => {}
        }
    }
    let response = prompt
        .await
        .expect("first prompt task should join")
        .expect("first prompt should complete");
    assert_eq!(
        response,
        RuntimeResponse::PromptCompleted {
            stop_reason: RuntimePromptStopReason::EndTurn,
        }
    );
}

async fn assert_state_sequence(
    events: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>,
    expected: &[RuntimeState],
) {
    for expected_state in expected {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("runtime event timed out")
                .expect("runtime event channel closed");
            if let RuntimeEvent::RuntimeStateChanged { state } = event {
                assert_eq!(state, *expected_state);
                break;
            }
        }
    }
}

async fn wait_for_runtime_state(
    events: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>,
    expected: RuntimeState,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                events.recv().await,
                Ok(RuntimeEvent::RuntimeStateChanged { state }) if state == expected
            ) {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for runtime state {expected:?}"));
}

async fn wait_for_heartbeat(path: &Path, minimum: u64) -> u64 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(contents) = fs::read_to_string(path)
                && let Ok(heartbeat) = contents.trim().parse::<u64>()
                && heartbeat > minimum
            {
                return heartbeat;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {}", path.display()))
}

async fn wait_for_stable_heartbeat(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let before = wait_for_heartbeat(path, 0).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        let after = fs::read_to_string(path)
            .ok()
            .and_then(|contents| contents.trim().parse::<u64>().ok());
        if after == Some(before) {
            return;
        }
    }
    panic!("runtime descendant continued after last owner was dropped");
}

fn fixture_permission_workspace() -> std::path::PathBuf {
    if cfg!(windows) {
        r"C:\fixture-workspace".into()
    } else {
        "/fixture-workspace".into()
    }
}
