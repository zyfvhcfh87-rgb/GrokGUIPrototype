use std::{process::Stdio, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

struct AgentProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl AgentProcess {
    fn spawn(scenario: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
            .arg(scenario)
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

    drop(agent.stdin);
    let status = timeout(Duration::from_secs(2), agent.child.wait())
        .await
        .expect("fake agent should exit after stdin closes")
        .expect("fake agent should be waitable");
    assert!(status.success());
}

#[tokio::test]
async fn lifecycle_manages_sessions_and_replays_before_resume_response() {
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
    let replay = agent.receive().await;
    assert_eq!(replay["method"], "session/update");
    assert_eq!(replay["params"]["sessionId"], "session-001");
    assert_eq!(
        replay["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
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
