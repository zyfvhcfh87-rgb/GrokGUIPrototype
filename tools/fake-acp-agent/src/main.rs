use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use clap::{Parser, ValueEnum};
use serde_json::{Value, json};

const SESSION_ID: &str = "session-001";
const FIXTURE_CWD: &str = r"C:\fixture-workspace";
const SENTINEL_READY_FILE: &str = "sentinel.ready";
const SENTINEL_HEARTBEAT_FILE: &str = "sentinel.heartbeat";
const SENTINEL_STOP_FILE: &str = "sentinel.stop";
const SENTINEL_STOPPED_FILE: &str = "sentinel.stopped";
const SENTINEL_MAX_LIFETIME: Duration = Duration::from_secs(15);
const DESCENDANT_EOF_LINGER: Duration = Duration::from_secs(10);
const PERSISTED_SESSION_MARKER: &[u8] = b"created\n";
const CLOSED_SESSION_MARKER: &[u8] = b"closed\n";
const REPEATED_LIST_CURSOR: &str = "fixture-page";
const OVERSIZED_LIST_CURSOR_BYTES: usize = 4 * 1024 + 1;

#[derive(Debug, Parser)]
struct Args {
    #[arg(value_enum, default_value_t = Scenario::Lifecycle)]
    scenario: Scenario,

    #[arg(long, value_name = "PATH")]
    state_file: Option<PathBuf>,

    #[arg(long, value_name = "DIRECTORY")]
    sentinel_dir: Option<PathBuf>,

    #[arg(long)]
    linger_after_eof: bool,

    #[arg(long, value_enum)]
    recovery_fault: Option<RecoveryFault>,

    #[arg(long, value_enum)]
    lifecycle_fault: Option<LifecycleFault>,

    #[arg(long, value_name = "PATH")]
    close_marker: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Scenario {
    Lifecycle,
    Malformed,
    Stderr,
    Crash,
    CrashAfterNew,
    Descendant,
    Sentinel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum RecoveryFault {
    RepeatedCursor,
    OversizedCursor,
    HangList,
    HangResume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LifecycleFault {
    PromptError,
    CancelError,
    CloseError,
    HangClose,
    HangPrompt,
    ListError,
    LoadError,
    ResumeError,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();
    let result = match args.scenario {
        Scenario::Lifecycle => run_lifecycle(
            false,
            args.state_file.as_deref(),
            false,
            args.recovery_fault,
            args.lifecycle_fault,
            args.close_marker.as_deref(),
        ),
        Scenario::Malformed => run_malformed(),
        Scenario::Stderr => run_lifecycle(
            true,
            args.state_file.as_deref(),
            false,
            args.recovery_fault,
            args.lifecycle_fault,
            args.close_marker.as_deref(),
        ),
        Scenario::Crash => run_crash(),
        Scenario::CrashAfterNew => required_path(args.state_file.as_deref(), "--state-file")
            .and_then(|path| {
                run_lifecycle(
                    false,
                    Some(path),
                    true,
                    args.recovery_fault,
                    args.lifecycle_fault,
                    args.close_marker.as_deref(),
                )
            }),
        Scenario::Descendant => required_path(args.sentinel_dir.as_deref(), "--sentinel-dir")
            .and_then(|directory| run_descendant(directory, args.linger_after_eof)),
        Scenario::Sentinel => {
            required_path(args.sentinel_dir.as_deref(), "--sentinel-dir").and_then(run_sentinel)
        }
    };

    if result.is_err() {
        eprintln!("fake-acp-agent: fixture I/O failed");
        std::process::exit(1);
    }
}

fn required_path<'a>(path: Option<&'a Path>, option: &str) -> io::Result<&'a Path> {
    path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("fixture scenario requires {option}"),
        )
    })
}

fn persisted_session_exists(state_file: Option<&Path>) -> io::Result<bool> {
    Ok(load_persisted_session(state_file)?.0)
}

fn load_persisted_session(state_file: Option<&Path>) -> io::Result<(bool, Option<String>)> {
    let Some(path) = state_file else {
        return Ok((false, None));
    };

    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((false, None)),
        Err(error) => return Err(error),
    };
    if !contents.starts_with(PERSISTED_SESSION_MARKER) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "fixture state contains an unexpected marker",
        ));
    }

    let rest = contents[PERSISTED_SESSION_MARKER.len()..].to_vec();
    if rest.is_empty() {
        return Ok((true, None));
    }
    let cwd = String::from_utf8(rest)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "fixture state contains a non-UTF-8 workspace",
            )
        })?
        .trim_end_matches(['\n', '\r'])
        .to_owned();
    if cwd.is_empty() {
        Ok((true, None))
    } else {
        Ok((true, Some(cwd)))
    }
}

fn persist_session(path: &Path, cwd: &str) -> io::Result<()> {
    if persisted_session_exists(Some(path))? {
        return Ok(());
    }

    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(PERSISTED_SESSION_MARKER)?;
    file.write_all(cwd.as_bytes())?;
    file.write_all(b"\n")
}

fn run_descendant(directory: &Path, linger_after_eof: bool) -> io::Result<()> {
    prepare_sentinel_directory(directory)?;
    let executable = std::env::current_exe()?;
    let mut sentinel = Command::new(executable)
        .arg("sentinel")
        .arg("--sentinel-dir")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let serve_result = serve_descendant_protocol();
    if linger_after_eof && serve_result.is_ok() {
        thread::sleep(DESCENDANT_EOF_LINGER);
    }
    let cleanup_result = stop_sentinel(directory, &mut sentinel);
    serve_result.and(cleanup_result)
}

fn prepare_sentinel_directory(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    for name in [
        SENTINEL_READY_FILE,
        SENTINEL_HEARTBEAT_FILE,
        SENTINEL_STOP_FILE,
        SENTINEL_STOPPED_FILE,
    ] {
        if directory.join(name).exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "sentinel directory contains fixture control files",
            ));
        }
    }
    Ok(())
}

fn serve_descendant_protocol() -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut line = String::new();

    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = frame.get("id").cloned() else {
            continue;
        };
        match frame.get("method").and_then(Value::as_str) {
            Some("initialize") => write_frame(&mut output, initialize_response(id))?,
            Some("authenticate") => write_frame(&mut output, result_response(id, json!({})))?,
            Some(_) => write_frame(
                &mut output,
                error_response(id, -32601, "Method unavailable in descendant fixture"),
            )?,
            None => {}
        }
    }
}

fn stop_sentinel(directory: &Path, sentinel: &mut Child) -> io::Result<()> {
    fs::write(directory.join(SENTINEL_STOP_FILE), b"stop\n")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if sentinel.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }

    sentinel.kill()?;
    sentinel.wait()?;
    Ok(())
}

fn run_sentinel(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    fs::write(
        directory.join(SENTINEL_READY_FILE),
        format!("{}\n", std::process::id()),
    )?;

    let started_at = Instant::now();
    let mut heartbeat = 0_u64;
    let stopped_reason = loop {
        if directory.join(SENTINEL_STOP_FILE).is_file() {
            break "requested\n";
        }
        if started_at.elapsed() >= SENTINEL_MAX_LIFETIME {
            break "ttl\n";
        }

        heartbeat += 1;
        fs::write(
            directory.join(SENTINEL_HEARTBEAT_FILE),
            format!("{heartbeat}\n"),
        )?;
        thread::sleep(Duration::from_millis(25));
    };

    fs::write(directory.join(SENTINEL_STOPPED_FILE), stopped_reason)
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

fn run_lifecycle(
    emit_diagnostic: bool,
    state_file: Option<&Path>,
    crash_after_new: bool,
    recovery_fault: Option<RecoveryFault>,
    lifecycle_fault: Option<LifecycleFault>,
    close_marker: Option<&Path>,
) -> io::Result<()> {
    if emit_diagnostic {
        eprintln!("fake-acp-agent: sanitized diagnostic fixture");
    }

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut line = String::new();
    let (session_created, persisted_cwd) = load_persisted_session(state_file)?;
    let mut state = LifecycleState {
        session_created,
        session_cwd: persisted_cwd,
        ..LifecycleState::default()
    };

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
                state.session_cwd = request_cwd(&frame);
                if let Some(path) = state_file {
                    persist_session(path, session_cwd(&state))?;
                }
                write_frame(
                    &mut output,
                    result_response(
                        id,
                        json!({
                            "sessionId": SESSION_ID,
                            "modes": {
                                "currentModeId": "default",
                                "availableModes": [
                                    { "id": "default", "name": "Default" },
                                    { "id": "plan", "name": "Plan" }
                                ]
                            },
                            "configOptions": fixture_config_options(false)
                        }),
                    ),
                )?;
                if crash_after_new {
                    eprintln!("fake-acp-agent: deterministic post-session crash fixture");
                    std::process::exit(86);
                }
            }
            "session/list" if state.authenticated => {
                if lifecycle_fault == Some(LifecycleFault::ListError) {
                    write_frame(
                        &mut output,
                        error_response(id, -32013, "Deterministic list failure"),
                    )?;
                    continue;
                }
                if recovery_fault == Some(RecoveryFault::HangList) {
                    continue;
                }
                if recovery_fault == Some(RecoveryFault::RepeatedCursor) {
                    write_frame(
                        &mut output,
                        result_response(
                            id,
                            json!({
                                "sessions": [],
                                "nextCursor": REPEATED_LIST_CURSOR
                            }),
                        ),
                    )?;
                    continue;
                }
                if recovery_fault == Some(RecoveryFault::OversizedCursor) {
                    write_frame(
                        &mut output,
                        result_response(
                            id,
                            json!({
                                "sessions": [],
                                "nextCursor": "x".repeat(OVERSIZED_LIST_CURSOR_BYTES)
                            }),
                        ),
                    )?;
                    continue;
                }
                let sessions = if state.session_created
                    && request_cwd(&frame)
                        .is_none_or(|cwd| cwd_matches(&cwd, session_cwd(&state)))
                {
                    json!([{
                        "sessionId": SESSION_ID,
                        "cwd": session_cwd(&state),
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
                if lifecycle_fault == Some(LifecycleFault::ResumeError) {
                    write_frame(
                        &mut output,
                        error_response(id, -32014, "Deterministic resume failure"),
                    )?;
                    continue;
                }
                if recovery_fault == Some(RecoveryFault::HangResume) {
                    continue;
                }
                if !requested_cwd_matches_session(&frame, &state) {
                    write_frame(
                        &mut output,
                        error_response(id, -32602, "Session does not belong to the requested workspace"),
                    )?;
                    continue;
                }
                state.session_open = true;
                write_frame(&mut output, result_response(id, json!({})))?;
            }
            "session/load" if state.authenticated && state.session_created => {
                if lifecycle_fault == Some(LifecycleFault::LoadError) {
                    write_frame(
                        &mut output,
                        error_response(id, -32015, "Deterministic load failure"),
                    )?;
                    continue;
                }
                if !requested_cwd_matches_session(&frame, &state) {
                    write_frame(
                        &mut output,
                        error_response(id, -32602, "Session does not belong to the requested workspace"),
                    )?;
                    continue;
                }
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
                if lifecycle_fault == Some(LifecycleFault::HangPrompt) && state.prompt_count == 1 {
                    continue;
                }
                if lifecycle_fault == Some(LifecycleFault::PromptError) && state.prompt_count == 1 {
                    write_frame(
                        &mut output,
                        error_response(id, -32010, "Deterministic prompt failure"),
                    )?;
                    continue;
                }
                if lifecycle_fault == Some(LifecycleFault::CancelError) && state.prompt_count == 2 {
                    write_frame(
                        &mut output,
                        error_response(id, -32011, "Deterministic cancel failure"),
                    )?;
                    continue;
                }
                if state.prompt_count == 1 {
                    run_prompt(&mut input, &mut output, id)?;
                } else {
                    run_cancellation_prompt(&mut input, &mut output, id)?;
                }
            }
            "session/set_model" if state.authenticated && state.session_open => {
                let valid_model = frame.pointer("/params/modelId").and_then(Value::as_str)
                    == Some("fixture-model");
                let valid_effort = frame
                    .pointer("/params/_meta/reasoningEffort")
                    .and_then(Value::as_str)
                    == Some("high");
                if valid_model && valid_effort {
                    write_frame(&mut output, grok_models_notification())?;
                    write_frame(
                        &mut output,
                        grok_session_notification(json!({
                            "sessionUpdate": "model_changed",
                            "model_id": "fixture-model",
                            "reasoning_effort": "high"
                        })),
                    )?;
                    write_frame(
                        &mut output,
                        result_response(id, json!({ "_meta": { "model": "fixture-model" } })),
                    )?;
                } else {
                    write_frame(
                        &mut output,
                        error_response(
                            id,
                            -32602,
                            "Expected advertised model and reasoning effort",
                        ),
                    )?;
                }
            }
            "session/set_mode" if state.authenticated && state.session_open => {
                let mode = frame.pointer("/params/modeId").and_then(Value::as_str);
                if matches!(mode, Some("plan" | "default")) {
                    write_frame(
                        &mut output,
                        session_update(json!({
                            "sessionUpdate": "current_mode_update",
                            "currentModeId": mode
                        })),
                    )?;
                    write_frame(&mut output, result_response(id, json!({})))?;
                } else {
                    write_frame(
                        &mut output,
                        error_response(id, -32602, "Unsupported fixture session mode"),
                    )?;
                }
            }
            "session/set_config_option" if state.authenticated && state.session_open => {
                let valid = frame.pointer("/params/configId").and_then(Value::as_str)
                    == Some("fixture-toggle")
                    && frame.pointer("/params/type").and_then(Value::as_str) == Some("boolean")
                    && frame.pointer("/params/value").and_then(Value::as_bool) == Some(true);
                if valid {
                    write_frame(
                        &mut output,
                        session_update(json!({
                            "sessionUpdate": "config_option_update",
                            "configOptions": fixture_config_options(true)
                        })),
                    )?;
                    write_frame(
                        &mut output,
                        result_response(
                            id,
                            json!({ "configOptions": fixture_config_options(true) }),
                        ),
                    )?;
                } else {
                    write_frame(
                        &mut output,
                        error_response(id, -32602, "Unsupported fixture config option"),
                    )?;
                }
            }
            "session/close" if state.authenticated && state.session_created => {
                if lifecycle_fault == Some(LifecycleFault::HangClose) {
                    state.session_open = false;
                    continue;
                }
                if lifecycle_fault == Some(LifecycleFault::CloseError) {
                    write_frame(
                        &mut output,
                        error_response(id, -32012, "Deterministic close failure"),
                    )?;
                    continue;
                }
                state.session_open = false;
                if let Some(path) = close_marker {
                    write_fixed_marker(path, CLOSED_SESSION_MARKER)?;
                }
                write_frame(&mut output, result_response(id, json!({})))?;
            }
            _ => write_frame(
                &mut output,
                error_response(id, -32601, "Method unavailable in fixture state"),
            )?,
        }
    }
}

fn write_fixed_marker(path: &Path, marker: &[u8]) -> io::Result<()> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => file.write_all(marker),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if fs::read(path)? == marker {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "fixture marker contains unexpected data",
                ))
            }
        }
        Err(error) => Err(error),
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
    session_cwd: Option<String>,
    prompt_count: u64,
}

fn request_cwd(frame: &Value) -> Option<String> {
    frame
        .pointer("/params/cwd")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn session_cwd(state: &LifecycleState) -> &str {
    state.session_cwd.as_deref().unwrap_or(FIXTURE_CWD)
}

fn requested_cwd_matches_session(frame: &Value, state: &LifecycleState) -> bool {
    request_cwd(frame).is_none_or(|cwd| cwd_matches(&cwd, session_cwd(state)))
}

fn cwd_matches(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left = left.trim_end_matches(['/', '\\']);
    let right = right.trim_end_matches(['/', '\\']);
    left == right || left.eq_ignore_ascii_case(right)
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
            },
            "_meta": {
                "modelState": {
                    "currentModelId": "fixture-model",
                    "availableModels": [{
                        "modelId": "fixture-model",
                        "name": "Fixture model",
                        "description": "Deterministic local fixture",
                        "_meta": {
                            "reasoningEffort": "high",
                            "reasoningEfforts": [
                                {
                                    "id": "low",
                                    "label": "Low",
                                    "description": "Fast fixture reasoning",
                                    "value": "low",
                                    "default": false
                                },
                                {
                                    "id": "high",
                                    "label": "High",
                                    "description": "Deliberate fixture reasoning",
                                    "value": "high",
                                    "default": true
                                }
                            ],
                            "supportsReasoningEffort": true,
                            "totalContextTokens": 4096
                        }
                    }]
                }
            }
        }
    })
}

fn fixture_config_options(current_value: bool) -> Value {
    json!([{
        "id": "fixture-toggle",
        "name": "Fixture toggle",
        "type": "boolean",
        "currentValue": current_value
    }])
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

fn grok_session_notification(update: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "_x.ai/session_notification",
        "params": {
            "sessionId": SESSION_ID,
            "update": update
        }
    })
}

fn grok_models_notification() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "_x.ai/models/update",
        "params": {
            "currentModelId": "fixture-model",
            "availableModels": [{
                "modelId": "fixture-model",
                "name": "Fixture Model",
                "reasoningEffort": "high",
                "reasoningEfforts": [{
                    "id": "high",
                    "label": "High",
                    "value": "high",
                    "isDefault": true
                }],
                "supportsReasoningEffort": true,
                "totalContextTokens": 4096
            }]
        }
    })
}

fn write_frame(output: &mut impl Write, frame: Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, &frame)?;
    output.write_all(b"\n")?;
    output.flush()
}
