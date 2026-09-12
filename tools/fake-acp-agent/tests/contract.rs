use std::{ffi::OsString, process::Stdio, time::Duration};

use grok_runtime::normalize_grok_session_response;
#[cfg(windows)]
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

#[cfg(windows)]
use tokio::time::sleep;
#[cfg(windows)]
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
    System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    },
};

struct AgentProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl AgentProcess {
    fn spawn(scenario: &str) -> Self {
        Self::spawn_args([OsString::from(scenario)])
    }

    fn spawn_args(args: impl IntoIterator<Item = OsString>) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("fake agent should spawn");

        let stdin = child.stdin.take().expect("stdin should be piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout should be piped")).lines();

        Self {
            child,
            stdin,
            stdout,
        }
    }

    async fn send(&mut self, frame: Value) {
        let mut bytes = serde_json::to_vec(&frame).expect("frame should serialize");
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .expect("frame should be written");
        self.stdin.flush().await.expect("stdin should flush");
    }

    async fn receive(&mut self) -> Value {
        let line = timeout(Duration::from_secs(2), self.stdout.next_line())
            .await
            .expect("fake agent timed out")
            .expect("stdout should be readable")
            .expect("fake agent closed stdout");
        serde_json::from_str(&line).expect("stdout must contain JSON-RPC only")
    }

    async fn initialize_authenticate_and_open_session(&mut self) {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": 1, "clientCapabilities": {} }
        }))
        .await;
        assert_eq!(self.receive().await["id"], 1);

        self.send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "authenticate",
            "params": { "methodId": "fixture_auth" }
        }))
        .await;
        assert_eq!(
            self.receive().await,
            json!({ "jsonrpc": "2.0", "id": 2, "result": {} })
        );

        self.send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/new",
            "params": { "cwd": "C:\\fixture-workspace", "mcpServers": [] }
        }))
        .await;
        assert_eq!(self.receive().await["result"]["sessionId"], "session-001");
    }

    async fn hang_first_prompt(&mut self) {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/prompt",
            "params": {
                "sessionId": "session-001",
                "prompt": [{ "type": "text", "text": "Hang until cancelled." }]
            }
        }))
        .await;
    }
}

fn hang_prompt_agent() -> AgentProcess {
    AgentProcess::spawn_args([
        OsString::from("lifecycle"),
        OsString::from("--lifecycle-fault"),
        OsString::from("hang-prompt"),
    ])
}

#[tokio::test]
async fn lifecycle_negotiates_v1_and_advertises_supported_callbacks() {
    let mut agent = AgentProcess::spawn("lifecycle");
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {
                    "elicitation": { "form": {} }
                }
            }
        }))
        .await;

    let response = agent.receive().await;
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["protocolVersion"], 1);
    assert_eq!(response["result"]["agentInfo"]["name"], "fake-acp-agent");
    assert!(response["result"]["agentCapabilities"]["sessionCapabilities"]["list"].is_object());
    assert!(response["result"]["agentCapabilities"]["sessionCapabilities"]["resume"].is_object());
    assert!(response["result"]["agentCapabilities"]["sessionCapabilities"]["close"].is_object());
    assert_eq!(response["result"]["authMethods"][0]["id"], "fixture_auth");

    let normalized = normalize_grok_session_response(response);
    let catalog = normalized
        .models
        .expect("fake initialize model state must cross the runtime boundary");
    assert_eq!(catalog.current_model_id, "fixture-model");
    assert_eq!(catalog.available_models.len(), 1);
    let model = &catalog.available_models[0];
    assert_eq!(model.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(model.reasoning_efforts.len(), 2);
    assert_eq!(model.reasoning_efforts[1].value, "high");
    assert!(model.reasoning_efforts[1].is_default);

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn lifecycle_resumes_without_replay() {
    let mut agent = AgentProcess::spawn("lifecycle");
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": 1, "clientCapabilities": {} }
        }))
        .await;
    assert_eq!(agent.receive().await["id"], 1);

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "authenticate",
            "params": { "methodId": "fixture_auth" }
        }))
        .await;
    assert_eq!(
        agent.receive().await,
        json!({ "jsonrpc": "2.0", "id": 2, "result": {} })
    );

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/new",
            "params": {
                "cwd": "C:\\fixture-workspace",
                "mcpServers": []
            }
        }))
        .await;
    assert_eq!(agent.receive().await["result"]["sessionId"], "session-001");

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/list",
            "params": {}
        }))
        .await;
    let list = agent.receive().await;
    assert_eq!(list["result"]["sessions"][0]["sessionId"], "session-001");
    assert_eq!(
        list["result"]["sessions"][0]["cwd"],
        "C:\\fixture-workspace"
    );

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/resume",
            "params": {
                "sessionId": "session-001",
                "cwd": "C:\\fixture-workspace",
                "mcpServers": []
            }
        }))
        .await;
    assert_eq!(
        agent.receive().await,
        json!({ "jsonrpc": "2.0", "id": 5, "result": {} })
    );

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "session/close",
            "params": { "sessionId": "session-001" }
        }))
        .await;
    assert_eq!(
        agent.receive().await,
        json!({ "jsonrpc": "2.0", "id": 6, "result": {} })
    );

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn lifecycle_keeps_sessions_inside_the_requested_workspace() {
    let mut agent = AgentProcess::spawn("lifecycle");
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": 1, "clientCapabilities": {} }
        }))
        .await;
    assert_eq!(agent.receive().await["id"], 1);

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "authenticate",
            "params": { "methodId": "fixture_auth" }
        }))
        .await;
    assert_eq!(agent.receive().await["id"], 2);

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/new",
            "params": {
                "cwd": "C:\\fixture-workspace",
                "mcpServers": []
            }
        }))
        .await;
    assert_eq!(agent.receive().await["result"]["sessionId"], "session-001");

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/list",
            "params": { "cwd": "C:\\other-workspace" }
        }))
        .await;
    assert_eq!(agent.receive().await["result"]["sessions"], json!([]));

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/resume",
            "params": {
                "sessionId": "session-001",
                "cwd": "C:\\other-workspace",
                "mcpServers": []
            }
        }))
        .await;
    assert_eq!(agent.receive().await["error"]["code"], -32602);

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "session/load",
            "params": {
                "sessionId": "session-001",
                "cwd": "C:\\other-workspace",
                "mcpServers": []
            }
        }))
        .await;
    assert_eq!(agent.receive().await["error"]["code"], -32602);

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "session/list",
            "params": { "cwd": "C:\\fixture-workspace" }
        }))
        .await;
    let list = agent.receive().await;
    assert_eq!(list["result"]["sessions"][0]["sessionId"], "session-001");
    assert_eq!(
        list["result"]["sessions"][0]["cwd"],
        "C:\\fixture-workspace"
    );

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn lifecycle_streams_prompt_updates_and_honors_callback_responses() {
    let mut agent = AgentProcess::spawn("lifecycle");
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": 1, "clientCapabilities": {} }
        }))
        .await;
    agent.receive().await;
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "authenticate",
            "params": { "methodId": "fixture_auth" }
        }))
        .await;
    agent.receive().await;
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/new",
            "params": { "cwd": "C:\\fixture-workspace", "mcpServers": [] }
        }))
        .await;
    agent.receive().await;

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/prompt",
            "params": {
                "sessionId": "session-001",
                "prompt": [{ "type": "text", "text": "Run the fixture lifecycle." }]
            }
        }))
        .await;

    let thought = agent.receive().await;
    assert_eq!(
        thought["params"]["update"]["sessionUpdate"],
        "agent_thought_chunk"
    );
    let message = agent.receive().await;
    assert_eq!(
        message["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    let commands = agent.receive().await;
    assert_eq!(
        commands["params"]["update"]["sessionUpdate"],
        "available_commands_update"
    );
    let plan = agent.receive().await;
    assert_eq!(plan["params"]["update"]["sessionUpdate"], "plan");
    assert_eq!(
        plan["params"]["update"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let tool = agent.receive().await;
    assert_eq!(tool["params"]["update"]["sessionUpdate"], "tool_call");
    assert_eq!(tool["params"]["update"]["toolCallId"], "tool-001");
    assert_eq!(tool["params"]["update"]["status"], "pending");

    let permission = agent.receive().await;
    assert_eq!(permission["id"], "permission-001");
    assert_eq!(permission["method"], "session/request_permission");
    assert_eq!(permission["params"]["sessionId"], "session-001");
    assert_eq!(permission["params"]["toolCall"]["toolCallId"], "tool-001");
    assert_eq!(permission["params"]["options"].as_array().unwrap().len(), 4);
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": "permission-001",
            "result": {
                "outcome": { "outcome": "selected", "optionId": "allow-once" }
            }
        }))
        .await;

    let running = agent.receive().await;
    assert_eq!(
        running["params"]["update"]["sessionUpdate"],
        "tool_call_update"
    );
    assert_eq!(running["params"]["update"]["status"], "in_progress");

    let elicitation = agent.receive().await;
    assert_eq!(elicitation["id"], "elicitation-001");
    assert_eq!(elicitation["method"], "elicitation/create");
    assert_eq!(elicitation["params"]["mode"], "form");
    assert_eq!(elicitation["params"]["sessionId"], "session-001");
    assert_eq!(
        elicitation["params"]["requestedSchema"]["properties"]["label"]["type"],
        "string"
    );
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": "elicitation-001",
            "result": {
                "action": "accept",
                "content": { "label": "sanitized-label" }
            }
        }))
        .await;

    let completed = agent.receive().await;
    assert_eq!(
        completed["params"]["update"]["sessionUpdate"],
        "tool_call_update"
    );
    assert_eq!(completed["params"]["update"]["status"], "completed");
    let final_message = agent.receive().await;
    assert_eq!(
        final_message["params"]["update"]["content"]["text"],
        "Permission and input were accepted safely."
    );
    assert_eq!(
        agent.receive().await,
        json!({ "jsonrpc": "2.0", "id": 4, "result": { "stopReason": "end_turn" } })
    );

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn paginated_session_list_returns_a_second_page() {
    let mut agent = AgentProcess::spawn_args([
        OsString::from("lifecycle"),
        OsString::from("--paginate-sessions"),
    ]);
    agent.initialize_authenticate_and_open_session().await;

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/list",
            "params": { "cwd": "C:\\fixture-workspace" }
        }))
        .await;
    let page_one = agent.receive().await;
    assert_eq!(
        page_one["result"]["sessions"][0]["sessionId"],
        "session-001"
    );
    assert_eq!(page_one["result"]["nextCursor"], "fixture-page-2");

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/list",
            "params": {
                "cwd": "C:\\fixture-workspace",
                "cursor": "fixture-page-2"
            }
        }))
        .await;
    let page_two = agent.receive().await;
    assert_eq!(
        page_two["result"]["sessions"][0]["sessionId"],
        "session-002"
    );
    assert!(page_two["result"]["nextCursor"].is_null());

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "session/list",
            "params": {
                "cwd": "C:\\fixture-workspace",
                "cursor": "unknown-cursor"
            }
        }))
        .await;
    assert_eq!(agent.receive().await["result"]["sessions"], json!([]));

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn hang_prompt_session_cancel_resolves_cancelled() {
    let mut agent = hang_prompt_agent();
    agent.initialize_authenticate_and_open_session().await;
    agent.hang_first_prompt().await;

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": "session-001" }
        }))
        .await;
    assert_eq!(
        agent.receive().await,
        json!({ "jsonrpc": "2.0", "id": 4, "result": { "stopReason": "cancelled" } })
    );

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn hang_prompt_cancel_request_notification_resolves_cancelled() {
    let mut agent = hang_prompt_agent();
    agent.initialize_authenticate_and_open_session().await;
    agent.hang_first_prompt().await;

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "method": "$/cancel_request",
            "params": { "id": 4 }
        }))
        .await;
    let cancelled = agent.receive().await;
    assert_eq!(
        cancelled,
        json!({ "jsonrpc": "2.0", "id": 4, "result": { "stopReason": "cancelled" } })
    );
    assert_ne!(cancelled["error"]["code"], -32800);

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn hang_prompt_cancel_request_request_resolves_cancelled() {
    let mut agent = hang_prompt_agent();
    agent.initialize_authenticate_and_open_session().await;
    agent.hang_first_prompt().await;

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "$/cancel_request",
            "params": { "id": 4 }
        }))
        .await;
    assert_eq!(
        agent.receive().await,
        json!({ "jsonrpc": "2.0", "id": 5, "result": {} })
    );
    let cancelled = agent.receive().await;
    assert_eq!(
        cancelled,
        json!({ "jsonrpc": "2.0", "id": 4, "result": { "stopReason": "cancelled" } })
    );
    assert_ne!(cancelled["error"]["code"], -32800);

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn hang_prompt_session_close_resolves_cancelled_then_close() {
    let mut agent = hang_prompt_agent();
    agent.initialize_authenticate_and_open_session().await;
    agent.hang_first_prompt().await;

    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/close",
            "params": { "sessionId": "session-001" }
        }))
        .await;
    assert_eq!(
        agent.receive().await,
        json!({ "jsonrpc": "2.0", "id": 4, "result": { "stopReason": "cancelled" } })
    );
    assert_eq!(
        agent.receive().await,
        json!({ "jsonrpc": "2.0", "id": 5, "result": {} })
    );

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn malformed_scenario_emits_one_deliberately_invalid_stdout_frame() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
        .arg("malformed")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("fake agent should spawn");
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n")
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let line = timeout(Duration::from_secs(2), stdout.next_line())
        .await
        .expect("malformed scenario timed out")
        .unwrap()
        .expect("malformed scenario should emit one line");
    assert_eq!(
        line,
        r#"{"jsonrpc":"2.0","method":"session/update","params":}"#
    );
    assert!(serde_json::from_str::<Value>(&line).is_err());

    drop(stdin);
    let status = timeout(Duration::from_secs(2), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn stderr_scenario_keeps_diagnostics_off_stdout_and_remains_usable() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
        .arg("stderr")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("fake agent should spawn");
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await.unwrap();
    stdin.flush().await.unwrap();

    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let stdout_line = timeout(Duration::from_secs(2), stdout.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&stdout_line).unwrap()["id"],
        1
    );

    let mut stderr = child.stderr.take().unwrap();
    drop(stdin);
    let status = timeout(Duration::from_secs(2), child.wait())
        .await
        .unwrap()
        .unwrap();
    let mut diagnostic = String::new();
    stderr.read_to_string(&mut diagnostic).await.unwrap();
    assert!(status.success());
    assert_eq!(diagnostic, "fake-acp-agent: sanitized diagnostic fixture\n");
}

#[tokio::test]
async fn crash_scenario_exits_nonzero_with_a_sanitized_diagnostic() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
        .arg("crash")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("fake agent should spawn");
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n")
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let status = timeout(Duration::from_secs(2), child.wait())
        .await
        .unwrap()
        .unwrap();
    let mut stdout_bytes = Vec::new();
    let mut diagnostic = String::new();
    stdout.read_to_end(&mut stdout_bytes).await.unwrap();
    stderr.read_to_string(&mut diagnostic).await.unwrap();
    assert_eq!(status.code(), Some(86));
    assert!(stdout_bytes.is_empty());
    assert_eq!(diagnostic, "fake-acp-agent: deterministic crash fixture\n");
}

#[tokio::test]
async fn crash_then_new_process_resumes_then_loads_persisted_session() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory should be created");
    let state_file = temporary_directory.path().join("session-state.json");
    let persistent_args = |scenario: &str| {
        [
            OsString::from(scenario),
            OsString::from("--state-file"),
            state_file.as_os_str().to_os_string(),
        ]
    };

    let mut crashed = AgentProcess::spawn_args(persistent_args("crash-after-new"));
    initialize_and_authenticate(&mut crashed).await;
    crashed
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/new",
            "params": { "cwd": "C:\\fixture-workspace", "mcpServers": [] }
        }))
        .await;
    assert_eq!(
        crashed.receive().await["result"]["sessionId"],
        "session-001"
    );
    let crash_status = timeout(Duration::from_secs(2), crashed.child.wait())
        .await
        .expect("post-session crash should be prompt")
        .expect("crashed process should be waitable");
    assert_eq!(crash_status.code(), Some(86));

    let mut recovered = AgentProcess::spawn_args(persistent_args("lifecycle"));
    initialize_and_authenticate(&mut recovered).await;
    recovered
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/list",
            "params": {}
        }))
        .await;
    assert_eq!(
        recovered.receive().await["result"]["sessions"][0]["sessionId"],
        "session-001"
    );

    recovered
        .send(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/resume",
            "params": {
                "sessionId": "session-001",
                "cwd": "C:\\fixture-workspace",
                "mcpServers": []
            }
        }))
        .await;
    assert_eq!(
        recovered.receive().await,
        json!({ "jsonrpc": "2.0", "id": 4, "result": {} })
    );

    recovered
        .send(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/load",
            "params": {
                "sessionId": "session-001",
                "cwd": "C:\\fixture-workspace",
                "mcpServers": []
            }
        }))
        .await;
    assert_eq!(
        recovered.receive().await["params"]["update"]["sessionUpdate"],
        "user_message_chunk"
    );
    assert_eq!(
        recovered.receive().await["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert_eq!(
        recovered.receive().await,
        json!({ "jsonrpc": "2.0", "id": 5, "result": {} })
    );

    recovered
        .send(json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "session/close",
            "params": { "sessionId": "session-001" }
        }))
        .await;
    assert_eq!(
        recovered.receive().await,
        json!({ "jsonrpc": "2.0", "id": 6, "result": {} })
    );
    drop(recovered.stdin);
    assert!(
        timeout(Duration::from_secs(2), recovered.child.wait())
            .await
            .expect("recovered process should stop after EOF")
            .expect("recovered process should be waitable")
            .success()
    );
}

async fn initialize_and_authenticate(agent: &mut AgentProcess) {
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": 1, "clientCapabilities": {} }
        }))
        .await;
    assert_eq!(agent.receive().await["result"]["protocolVersion"], 1);
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "authenticate",
            "params": { "methodId": "fixture_auth" }
        }))
        .await;
    assert_eq!(
        agent.receive().await,
        json!({ "jsonrpc": "2.0", "id": 2, "result": {} })
    );
}

#[cfg(windows)]
#[tokio::test]
async fn graceful_parent_eof_stops_its_test_descendant() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory should be created");
    let sentinel_directory = temporary_directory.path().join("sentinel");
    let mut cleanup = SentinelCleanup::new(sentinel_directory.clone());
    let mut agent = AgentProcess::spawn_args(descendant_args(&sentinel_directory));

    initialize_descendant(&mut agent).await;
    cleanup.arm(wait_for_pid(&sentinel_directory.join("sentinel.ready")).await);
    wait_for_counter_greater(&sentinel_directory.join("sentinel.heartbeat"), 0).await;

    drop(agent.stdin);
    let parent_status = timeout(Duration::from_secs(4), agent.child.wait())
        .await
        .expect("descendant fixture should stop after EOF")
        .expect("descendant fixture should be waitable");
    assert!(parent_status.success());
    assert_eq!(
        wait_for_text(&sentinel_directory.join("sentinel.stopped")).await,
        "requested"
    );
    cleanup.disarm();
}

#[cfg(windows)]
#[tokio::test]
async fn abrupt_parent_exit_exposes_orphan_and_test_reaps_its_own_sentinel() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory should be created");
    let sentinel_directory = temporary_directory.path().join("sentinel");
    let mut cleanup = SentinelCleanup::new(sentinel_directory.clone());
    let mut agent = AgentProcess::spawn_args(descendant_args(&sentinel_directory));

    initialize_descendant(&mut agent).await;
    cleanup.arm(wait_for_pid(&sentinel_directory.join("sentinel.ready")).await);
    let before = wait_for_counter_greater(&sentinel_directory.join("sentinel.heartbeat"), 0).await;

    agent
        .child
        .kill()
        .await
        .expect("test should be able to terminate its fixture parent");
    let parent_status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("terminated fixture parent should stop")
        .expect("terminated fixture parent should be waitable");
    assert!(!parent_status.success());

    let after =
        wait_for_counter_greater(&sentinel_directory.join("sentinel.heartbeat"), before).await;
    assert!(
        after > before,
        "heartbeat must advance after the parent dies to prove the descendant survived"
    );

    assert_eq!(cleanup.stop_and_wait().await, "requested");
}

#[cfg(windows)]
fn descendant_args(directory: &Path) -> [OsString; 3] {
    [
        OsString::from("descendant"),
        OsString::from("--sentinel-dir"),
        directory.as_os_str().to_os_string(),
    ]
}

#[cfg(windows)]
async fn initialize_descendant(agent: &mut AgentProcess) {
    agent
        .send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": 1, "clientCapabilities": {} }
        }))
        .await;
    assert_eq!(agent.receive().await["result"]["protocolVersion"], 1);
}

#[cfg(windows)]
async fn wait_for_pid(path: &Path) -> u32 {
    wait_for_text(path)
        .await
        .parse()
        .expect("sentinel ready file should contain its process identifier")
}

#[cfg(windows)]
async fn wait_for_text(path: &Path) -> String {
    timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(contents) = fs::read_to_string(path) {
                return contents.trim().to_owned();
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {}", path.display()))
}

#[cfg(windows)]
async fn wait_for_counter_greater(path: &Path, minimum: u64) -> u64 {
    timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(contents) = fs::read_to_string(path)
                && let Ok(value) = contents.trim().parse::<u64>()
                && value > minimum
            {
                return value;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {} to advance", path.display()))
}

#[cfg(windows)]
struct SentinelCleanup {
    directory: PathBuf,
    handle: Option<HANDLE>,
}

#[cfg(windows)]
impl SentinelCleanup {
    fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            handle: None,
        }
    }

    fn arm(&mut self, pid: u32) {
        assert!(self.handle.is_none(), "sentinel cleanup must only arm once");
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) }
            .expect("test should open its own sentinel process");
        self.handle = Some(handle);
    }

    fn disarm(&mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe { CloseHandle(handle) }.expect("sentinel process handle should close");
        }
    }

    async fn stop_and_wait(&mut self) -> String {
        fs::write(self.directory.join("sentinel.stop"), b"stop\n")
            .expect("test should be able to request sentinel shutdown");
        let reason = wait_for_text(&self.directory.join("sentinel.stopped")).await;
        let handle = self.handle.expect("sentinel cleanup should be armed");
        wait_for_sentinel_exit(handle).await;
        self.disarm();
        reason
    }
}

#[cfg(windows)]
async fn wait_for_sentinel_exit(handle: HANDLE) {
    timeout(Duration::from_secs(3), async {
        loop {
            let wait = unsafe { WaitForSingleObject(handle, 0) };
            if wait == WAIT_OBJECT_0 {
                return;
            }
            assert_ne!(wait, WAIT_FAILED, "waiting for sentinel process failed");
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("sentinel process should exit after its stop acknowledgement");
}

#[cfg(windows)]
impl Drop for SentinelCleanup {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };

        let _ = fs::write(self.directory.join("sentinel.stop"), b"stop\n");
        for _ in 0..100 {
            let wait = unsafe { WaitForSingleObject(handle, 0) };
            if wait == WAIT_OBJECT_0 {
                unsafe { CloseHandle(handle) }.ok();
                return;
            }
            if wait == WAIT_FAILED {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        unsafe { TerminateProcess(handle, 1) }.ok();
        let _ = unsafe { WaitForSingleObject(handle, 2_000) };
        unsafe { CloseHandle(handle) }.ok();
    }
}
