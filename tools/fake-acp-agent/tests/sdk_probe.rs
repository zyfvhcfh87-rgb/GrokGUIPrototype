use std::time::Duration;

use grok_acp_probe::{LifecycleOptions, ProbeKind, ProbeTarget, run_probe};
use serde_json::Value;

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
