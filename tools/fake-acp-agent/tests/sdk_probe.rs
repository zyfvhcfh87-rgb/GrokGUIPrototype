use std::time::Duration;

use grok_acp_probe::{ControlOptions, LifecycleOptions, ProbeKind, ProbeTarget, run_probe};
use serde_json::Value;
use tempfile::TempDir;

#[tokio::test]
async fn official_sdk_probe_completes_the_phase_zero_lifecycle() {
    let target = ProbeTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"), ["lifecycle"]);
    let report = run_probe(
        target,
        ProbeKind::Lifecycle(LifecycleOptions {
            workspace: std::env::current_dir().expect("current directory should be available"),
            prompt: "Run the deterministic fixture lifecycle.".to_owned(),
            auth_method: Some("fixture_auth".to_owned()),
            exercise_cancel: true,
        }),
        Duration::from_secs(10),
    )
    .await
    .expect("the official SDK should complete the fake lifecycle");

    let report = serde_json::to_value(report).expect("probe report should serialize");
    for expected in [
        "initialize",
        "authenticate",
        "session/new",
        "session/prompt",
        "session/cancel",
        "session/list",
        "session/load",
        "session/resume",
        "session/close",
    ] {
        assert_confirmed_step(&report, expected);
    }

    assert_eq!(report["eventCounts"]["permission_requested"], 1);
    assert_eq!(report["eventCounts"]["elicitation_requested"], 1);
    assert_eq!(report["eventCounts"]["tool_call"], 1);
    assert_eq!(report["eventCounts"]["plan_changed"], 1);
    assert!(
        report["eventCounts"]["agent_message_chunk"]
            .as_u64()
            .unwrap_or(0)
            >= 3
    );
    assert!(report["eventCounts"]["thought_chunk"].as_u64().unwrap_or(0) >= 2);
    assert_eq!(report["wire"]["malformedStdoutLines"], 0);
}

#[tokio::test]
async fn official_sdk_probe_exercises_session_local_controls_without_a_model_turn() {
    let target = ProbeTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"), ["lifecycle"]);
    let report = run_probe(
        target,
        ProbeKind::Controls(ControlOptions {
            workspace: std::env::current_dir().expect("current directory should be available"),
            auth_method: Some("fixture_auth".to_owned()),
        }),
        Duration::from_secs(10),
    )
    .await
    .expect("the official SDK should complete the fake control probe");

    let report = serde_json::to_value(report).expect("probe report should serialize");
    for expected in [
        "initialize",
        "authenticate",
        "session/new",
        "session/set_model",
        "session/set_mode:plan",
        "session/set_mode:default",
        "session/close",
    ] {
        assert_confirmed_step(&report, expected);
    }

    assert_eq!(
        report["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["step"] == "session/set_config_option")
            .unwrap()["status"],
        "skipped"
    );
    assert!(
        report["wire"]["clientMethods"]
            .as_array()
            .unwrap()
            .iter()
            .all(|method| method != "session/prompt")
    );
    assert!(
        report["wire"]["agentMethods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|method| method == "_x.ai/session_notification")
    );
    let session_new = report["wire"]["frames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|frame| frame["method"] == "session/new")
        .expect("session/new wire frame");
    assert!(
        session_new["fieldPaths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "params._meta.reasoningEffort")
    );
}

#[tokio::test]
async fn unadvertised_requested_auth_method_fails_with_partial_initialize_evidence() {
    let error = run_probe(
        ProbeTarget::new(env!("CARGO_BIN_EXE_fake-acp-agent"), ["lifecycle"]),
        ProbeKind::Lifecycle(LifecycleOptions {
            workspace: std::env::current_dir().expect("current directory should be available"),
            prompt: "This prompt must not run.".to_owned(),
            auth_method: Some("not-advertised".to_owned()),
            exercise_cancel: false,
        }),
        Duration::from_secs(10),
    )
    .await
    .expect_err("an explicit unknown auth method must fail closed");

    let report = serde_json::to_value(
        error
            .partial_report()
            .expect("initialize evidence should survive auth selection failure"),
    )
    .expect("partial report should serialize");
    assert_confirmed_step(&report, "initialize");
    assert_failed_step(&report, "authenticate");
    assert!(
        !report["steps"]
            .as_array()
            .expect("steps should be an array")
            .iter()
            .any(|step| step["step"] == "session/new")
    );
    assert!(
        error
            .to_string()
            .contains("requested_auth_method_not_advertised")
    );
}

#[tokio::test]
async fn prompt_failure_returns_sanitized_partial_evidence_after_closing() {
    assert_failed_lifecycle_preserves_report("prompt-error", "session/prompt").await;
}

#[tokio::test]
async fn cancel_failure_returns_sanitized_partial_evidence_after_closing() {
    assert_failed_lifecycle_preserves_report("cancel-error", "session/cancel").await;
}

#[tokio::test]
async fn close_failure_returns_sanitized_partial_evidence_and_non_success() {
    let temporary_directory = TempDir::new().expect("temporary directory");
    let close_marker = temporary_directory.path().join("closed.marker");
    let error = run_probe(
        fault_target("close-error", &close_marker),
        lifecycle_options(true),
        Duration::from_secs(10),
    )
    .await
    .expect_err("a close error must make the compatibility probe fail");

    let report = serde_json::to_value(
        error
            .partial_report()
            .expect("a post-initialize failure should retain its sanitized report"),
    )
    .expect("partial report should serialize");
    assert_confirmed_step(&report, "session/prompt");
    assert_failed_step(&report, "session/close");
    assert!(
        !close_marker.exists(),
        "the rejected close must not write the marker"
    );
    assert!(error.to_string().contains("session_close_request_failed"));
    assert!(
        !serde_json::to_string(&report)
            .expect("partial report should serialize")
            .contains("Deterministic close failure")
    );
}

#[tokio::test]
async fn exercise_timeout_reserves_time_for_session_close() {
    let temporary_directory = TempDir::new().expect("temporary directory");
    let close_marker = temporary_directory.path().join("closed.marker");
    let error = run_probe(
        fault_target("hang-prompt", &close_marker),
        lifecycle_options(false),
        Duration::from_secs(1),
    )
    .await
    .expect_err("a hung prompt must make the compatibility probe fail");

    let report = serde_json::to_value(
        error
            .partial_report()
            .expect("the timed-out exercise should retain its sanitized report"),
    )
    .expect("partial report should serialize");
    assert_failed_step(&report, "session/exercise");
    assert_confirmed_step(&report, "session/close");
    assert_eq!(
        std::fs::read(&close_marker).expect("reserved cleanup time should reach session/close"),
        b"closed\n"
    );
    assert!(error.to_string().contains("session_exercise_timed_out"));
}

async fn assert_failed_lifecycle_preserves_report(fault: &str, expected_failed_step: &str) {
    let temporary_directory = TempDir::new().expect("temporary directory");
    let close_marker = temporary_directory.path().join("closed.marker");
    let error = run_probe(
        fault_target(fault, &close_marker),
        lifecycle_options(true),
        Duration::from_secs(10),
    )
    .await
    .expect_err("the injected request failure must make the compatibility probe fail");

    let report = serde_json::to_value(
        error
            .partial_report()
            .expect("a post-initialize failure should retain its sanitized report"),
    )
    .expect("partial report should serialize");
    assert_confirmed_step(&report, "initialize");
    assert_confirmed_step(&report, "authenticate");
    assert_confirmed_step(&report, "session/new");
    assert_failed_step(&report, expected_failed_step);
    assert_confirmed_step(&report, "session/close");
    assert_eq!(
        std::fs::read(close_marker).expect("cleanup should close the fake session"),
        b"closed\n"
    );
    let serialized = serde_json::to_string(&report).expect("partial report should serialize");
    assert!(!serialized.contains("Deterministic prompt failure"));
    assert!(!serialized.contains("Deterministic cancel failure"));
}

fn fault_target(fault: &str, close_marker: &std::path::Path) -> ProbeTarget {
    ProbeTarget::new(
        env!("CARGO_BIN_EXE_fake-acp-agent"),
        [
            "lifecycle".to_owned(),
            "--lifecycle-fault".to_owned(),
            fault.to_owned(),
            "--close-marker".to_owned(),
            close_marker.to_string_lossy().into_owned(),
        ],
    )
}

fn lifecycle_options(exercise_cancel: bool) -> ProbeKind {
    ProbeKind::Lifecycle(LifecycleOptions {
        workspace: std::env::current_dir().expect("current directory should be available"),
        prompt: "Run the deterministic fixture lifecycle.".to_owned(),
        auth_method: Some("fixture_auth".to_owned()),
        exercise_cancel,
    })
}

fn assert_confirmed_step(report: &Value, expected: &str) {
    let step = report["steps"]
        .as_array()
        .expect("steps should be an array")
        .iter()
        .find(|step| step["step"] == expected)
        .unwrap_or_else(|| panic!("missing lifecycle step {expected}"));
    assert_eq!(
        step["status"], "confirmed",
        "step {expected} was not confirmed"
    );
}

fn assert_failed_step(report: &Value, expected: &str) {
    let step = report["steps"]
        .as_array()
        .expect("steps should be an array")
        .iter()
        .find(|step| step["step"] == expected)
        .unwrap_or_else(|| panic!("missing lifecycle step {expected}"));
    assert_eq!(step["status"], "failed", "step {expected} did not fail");
}
