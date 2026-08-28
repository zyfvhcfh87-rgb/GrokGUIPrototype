use std::io::{self, BufRead, Write};

use clap::{Parser, ValueEnum};
use serde_json::{Value, json};

const SESSION_ID: &str = "session-001";
const FIXTURE_CWD: &str = r"C:\fixture-workspace";

#[derive(Debug, Parser)]
struct Args {
    #[arg(value_enum, default_value_t = Scenario::Lifecycle)]
    scenario: Scenario,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Scenario {
    Lifecycle,
    Malformed,
    Stderr,
    Crash,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let result = match Args::parse().scenario {
        Scenario::Lifecycle => run_lifecycle(false),
        Scenario::Malformed => run_malformed(),
        Scenario::Stderr => run_lifecycle(true),
        Scenario::Crash => run_crash(),
    };

    if result.is_err() {
        eprintln!("fake-acp-agent: fixture I/O failed");
        std::process::exit(1);
    }
}

fn run_malformed() -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut ignored_request = String::new();
    input.read_line(&mut ignored_request)?;

    let stdout = io::stdout();
    let mut output = stdout.lock();
    output.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":}\n")?;
    output.flush()
}

fn run_crash() -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut ignored_request = String::new();
    input.read_line(&mut ignored_request)?;

    let stderr = io::stderr();
    let mut diagnostic = stderr.lock();
    diagnostic.write_all(b"fake-acp-agent: deterministic crash fixture\n")?;
    diagnostic.flush()?;
    std::process::exit(86);
}

fn run_lifecycle(emit_diagnostic: bool) -> io::Result<()> {
    if emit_diagnostic {
        eprintln!("fake-acp-agent: sanitized diagnostic fixture");
    }

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut line = String::new();
    let mut state = LifecycleState::default();

    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }

        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            continue;
        };
        let Some(id) = frame.get("id").cloned() else {
            continue;
        };

        match method {
            "initialize" => {
                state.initialized = true;
                write_frame(&mut output, initialize_response(id))?;
            }
            "authenticate" if state.initialized => {
                if frame.pointer("/params/methodId").and_then(Value::as_str) == Some("fixture_auth")
                {
                    state.authenticated = true;
                    write_frame(&mut output, result_response(id, json!({})))?;
                } else {
                    write_frame(
                        &mut output,
                        error_response(id, -32602, "Unsupported fixture authentication method"),
                    )?;
                }
            }
            "session/new" if state.authenticated => {
                state.session_created = true;
                state.session_open = true;
                write_frame(
                    &mut output,
                    result_response(id, json!({ "sessionId": SESSION_ID })),
                )?;
            }
            "session/list" if state.authenticated => {
                let sessions = if state.session_created {
                    json!([{
                        "sessionId": SESSION_ID,
                        "cwd": FIXTURE_CWD,
                        "title": "Sanitized fixture session",
                        "updatedAt": "2030-01-01T00:00:00Z"
                    }])
                } else {
                    json!([])
                };
                write_frame(
                    &mut output,
                    result_response(id, json!({ "sessions": sessions })),
                )?;
            }
            "session/resume" if state.authenticated && state.session_created => {
                state.session_open = true;
                write_frame(
                    &mut output,
                    session_update(json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": {
                            "type": "text",
                            "text": "Replayed sanitized session context."
                        },
                        "messageId": "message-replay-001"
                    })),
                )?;
                write_frame(&mut output, result_response(id, json!({})))?;
            }
            "session/load" if state.authenticated && state.session_created => {
                state.session_open = true;
                write_frame(
                    &mut output,
                    session_update(json!({
                        "sessionUpdate": "user_message_chunk",
                        "content": {
                            "type": "text",
                            "text": "Replayed sanitized user prompt."
                        },
                        "messageId": "message-replay-user-001"
                    })),
                )?;
                write_frame(
                    &mut output,
                    session_update(agent_message(
                        "Replayed sanitized agent response.",
                        "message-replay-agent-001",
                    )),
                )?;
                write_frame(&mut output, result_response(id, json!({})))?;
            }
            "session/prompt" if state.authenticated && state.session_open => {
                state.prompt_count += 1;
                if state.prompt_count == 1 {
                    run_prompt(&mut input, &mut output, id)?;
                } else {
                    run_cancellation_prompt(&mut input, &mut output, id)?;
                }
            }
            "session/close" if state.authenticated && state.session_created => {
                state.session_open = false;
                write_frame(&mut output, result_response(id, json!({})))?;
            }
            _ => write_frame(
                &mut output,
                error_response(id, -32601, "Method unavailable in fixture state"),
            )?,
        }
    }
}

fn run_prompt(
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt_id: Value,
) -> io::Result<()> {
    for update in initial_prompt_updates() {
        write_frame(output, session_update(update))?;
    }

    write_frame(output, permission_request())?;
    let permission = match wait_for_callback(input, output, "permission-001")? {
        CallbackWait::Response(frame) => permission_decision(&frame),
        CallbackWait::Cancelled | CallbackWait::EndOfInput => {
            finish_cancelled_prompt(output, prompt_id)?;
            return Ok(());
        }
    };

    if permission == PermissionDecision::Cancelled {
        finish_cancelled_prompt(output, prompt_id)?;
        return Ok(());
    }

    if permission == PermissionDecision::Allowed {
        write_frame(
            output,
            session_update(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tool-001",
                "status": "in_progress"
            })),
        )?;
    }

    write_frame(output, elicitation_request())?;
    let elicitation_accepted = match wait_for_callback(input, output, "elicitation-001")? {
        CallbackWait::Response(frame) => elicitation_was_accepted(&frame),
        CallbackWait::Cancelled | CallbackWait::EndOfInput => {
            finish_cancelled_prompt(output, prompt_id)?;
            return Ok(());
        }
    };

    let (status, text, raw_output) = match (permission, elicitation_accepted) {
        (PermissionDecision::Allowed, true) => (
            "completed",
            "Permission and input were accepted safely.",
            json!({ "ok": true }),
        ),
        (PermissionDecision::Rejected, _) => (
            "failed",
            "Permission was denied; no operation was performed.",
            json!({ "ok": false, "reason": "permission_denied" }),
        ),
        (PermissionDecision::Allowed, false) => (
            "failed",
            "Input was not accepted; no operation was performed.",
            json!({ "ok": false, "reason": "input_not_accepted" }),
        ),
        (PermissionDecision::Cancelled, _) => unreachable!("cancelled prompts return early"),
    };

    write_frame(
        output,
        session_update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tool-001",
            "status": status,
            "rawOutput": raw_output
        })),
    )?;
    write_frame(
        output,
        session_update(agent_message(text, "message-agent-002")),
    )?;
    write_frame(
        output,
        result_response(prompt_id, json!({ "stopReason": "end_turn" })),
    )
}

fn initial_prompt_updates() -> [Value; 4] {
    [
        json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": {
                "type": "text",
                "text": "Considering the deterministic fixture."
            },
            "messageId": "message-thought-001"
        }),
        agent_message("I will inspect the sanitized fixture.", "message-agent-001"),
        json!({
            "sessionUpdate": "plan",
            "entries": [
                {
                    "content": "Request scoped permission",
                    "priority": "high",
                    "status": "in_progress"
                },
                {
                    "content": "Report the deterministic result",
                    "priority": "medium",
                    "status": "pending"
                }
            ]
        }),
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tool-001",
            "title": "Run sanitized fixture command",
            "kind": "execute",
            "status": "pending",
            "locations": [{
                "path": "C:\\fixture-workspace\\fixture.txt",
                "line": 1
            }],
            "rawInput": {
                "command": "fixture-tool --check fixture.txt",
                "cwd": FIXTURE_CWD,
                "affectedPath": "C:\\fixture-workspace\\fixture.txt"
            }
        }),
    ]
}

fn agent_message(text: &str, message_id: &str) -> Value {
    json!({
        "sessionUpdate": "agent_message_chunk",
        "content": { "type": "text", "text": text },
        "messageId": message_id
    })
}

fn permission_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "permission-001",
        "method": "session/request_permission",
        "params": {
            "sessionId": SESSION_ID,
            "toolCall": {
                "toolCallId": "tool-001",
                "title": "Run sanitized fixture command",
                "kind": "execute",
                "status": "pending",
                "locations": [{
                    "path": "C:\\fixture-workspace\\fixture.txt",
                    "line": 1
                }],
                "rawInput": {
                    "command": "fixture-tool --check fixture.txt",
                    "cwd": FIXTURE_CWD,
                    "affectedPath": "C:\\fixture-workspace\\fixture.txt"
                }
            },
            "options": [
                { "optionId": "allow-once", "name": "Allow once", "kind": "allow_once" },
                { "optionId": "allow-always", "name": "Always allow", "kind": "allow_always" },
                { "optionId": "reject-once", "name": "Reject once", "kind": "reject_once" },
                { "optionId": "reject-always", "name": "Always reject", "kind": "reject_always" }
            ]
        }
    })
}

fn elicitation_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "elicitation-001",
        "method": "elicitation/create",
        "params": {
            "mode": "form",
            "sessionId": SESSION_ID,
            "toolCallId": "tool-001",
            "message": "Provide a non-sensitive label for the fixture result.",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "label": {
                        "type": "string",
                        "title": "Fixture label",
                        "minLength": 1,
                        "maxLength": 64
                    }
                },
                "required": ["label"]
            }
        }
    })
}

enum CallbackWait {
    Response(Value),
    Cancelled,
    EndOfInput,
}

fn wait_for_callback(
    input: &mut impl BufRead,
    output: &mut impl Write,
    expected_id: &str,
) -> io::Result<CallbackWait> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(CallbackWait::EndOfInput);
        }

        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if frame.get("method").and_then(Value::as_str) == Some("session/cancel")
            && frame.pointer("/params/sessionId").and_then(Value::as_str) == Some(SESSION_ID)
        {
            return Ok(CallbackWait::Cancelled);
        }

        if frame.get("id").and_then(Value::as_str) == Some(expected_id)
            && frame.get("method").is_none()
        {
            return Ok(CallbackWait::Response(frame));
        }

        if let (Some(id), Some(_method)) = (frame.get("id"), frame.get("method")) {
            write_frame(
                output,
                error_response(id.clone(), -32001, "Fixture prompt is already active"),
            )?;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PermissionDecision {
    Allowed,
    Rejected,
    Cancelled,
}

fn permission_decision(frame: &Value) -> PermissionDecision {
    match frame
        .pointer("/result/outcome/outcome")
        .and_then(Value::as_str)
    {
        Some("selected") => {
            match frame
                .pointer("/result/outcome/optionId")
                .and_then(Value::as_str)
            {
                Some("allow-once" | "allow-always") => PermissionDecision::Allowed,
                Some("reject-once" | "reject-always") | None => PermissionDecision::Rejected,
                Some(_) => PermissionDecision::Rejected,
            }
        }
        Some("cancelled") => PermissionDecision::Cancelled,
        Some(_) | None => PermissionDecision::Rejected,
    }
}

fn elicitation_was_accepted(frame: &Value) -> bool {
    if frame.pointer("/result/action").and_then(Value::as_str) != Some("accept") {
        return false;
    }

    frame
        .pointer("/result/content/label")
        .and_then(Value::as_str)
        .is_some_and(|label| {
            !label.is_empty() && label.len() <= 64 && !label.chars().any(char::is_control)
        })
}

fn finish_cancelled_prompt(output: &mut impl Write, prompt_id: Value) -> io::Result<()> {
    write_frame(
        output,
        session_update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tool-001",
            "status": "failed",
            "rawOutput": { "ok": false, "reason": "cancelled" }
        })),
    )?;
    write_frame(
        output,
        result_response(prompt_id, json!({ "stopReason": "cancelled" })),
    )
}

#[derive(Default)]
struct LifecycleState {
    initialized: bool,
    authenticated: bool,
    session_created: bool,
    session_open: bool,
    prompt_count: u64,
}

fn run_cancellation_prompt(
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt_id: Value,
) -> io::Result<()> {
    write_frame(
        output,
        session_update(json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": {
                "type": "text",
                "text": "Waiting for the deterministic cancellation notification."
            },
            "messageId": "message-thought-cancel-001"
        })),
    )?;

    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return finish_cancelled_prompt(output, prompt_id);
        }

        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if frame.get("method").and_then(Value::as_str) == Some("session/cancel")
            && frame.pointer("/params/sessionId").and_then(Value::as_str) == Some(SESSION_ID)
        {
            write_frame(
                output,
                session_update(agent_message(
                    "Cancellation acknowledged safely.",
                    "message-agent-cancel-001",
                )),
            )?;
            return finish_cancelled_prompt(output, prompt_id);
        }

        if let (Some(id), Some(_method)) = (frame.get("id"), frame.get("method")) {
            write_frame(
                output,
                error_response(id.clone(), -32001, "Fixture prompt is already active"),
            )?;
        }
    }
}

fn initialize_response(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {
                    "image": false,
                    "audio": false,
                    "embeddedContext": false
                },
                "mcpCapabilities": {
                    "http": false,
                    "sse": false
                },
                "sessionCapabilities": {
                    "list": {},
                    "resume": {},
                    "close": {}
                },
                "auth": {}
            },
            "authMethods": [{
                "id": "fixture_auth",
                "name": "Fixture authentication",
                "description": "Deterministic authentication without credentials"
            }],
            "agentInfo": {
                "name": "fake-acp-agent",
                "title": "Fake ACP Agent",
                "version": "0.1.0"
            }
        }
    })
}

fn result_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn session_update(update: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": SESSION_ID,
            "update": update
        }
    })
}

fn write_frame(output: &mut impl Write, frame: Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, &frame)?;
    output.write_all(b"\n")?;
    output.flush()
}
