#![cfg(windows)]

use std::{fs, path::Path, time::Duration};

use grok_acp_probe::{ProbeError, ProbeTarget, run_managed_restart_probe};
use tokio::time::timeout;

const PROBE_DEADLINE: Duration = Duration::from_secs(8);
const OUTER_TEST_TIMEOUT: Duration = Duration::from_secs(30);
const SESSION_STATE_MARKER: &[u8] = b"created\n";
const SESSION_CLOSE_MARKER: &[u8] = b"closed\n";

#[tokio::test]
async fn managed_restart_rejects_a_repeated_cursor_and_still_closes_the_session() {
    assert_recovery_fault("repeated-cursor", "session_list_cursor_repeated").await;
}

#[tokio::test]
async fn managed_restart_rejects_an_oversized_cursor_and_still_closes_the_session() {
    assert_recovery_fault("oversized-cursor", "session_list_cursor_too_large").await;
}

#[tokio::test]
async fn managed_restart_times_out_a_hung_list_and_still_closes_the_session() {
    assert_recovery_fault("hang-list", "session_recovery_timed_out").await;
}

#[tokio::test]
async fn managed_restart_times_out_a_hung_resume_and_still_closes_the_session() {
    assert_recovery_fault("hang-resume", "session_recovery_timed_out").await;
}

async fn assert_recovery_fault(fault: &str, expected_error: &'static str) {
    let temporary_directory = tempfile::tempdir().expect("temporary directory should be created");
    let state_file = temporary_directory.path().join("session-created.marker");
    let close_marker = temporary_directory.path().join("session-closed.marker");
    let target = ProbeTarget::new(
        env!("CARGO_BIN_EXE_fake-acp-agent"),
        [
            "lifecycle".to_owned(),
            "--state-file".to_owned(),
            path_argument(&state_file),
            "--recovery-fault".to_owned(),
            fault.to_owned(),
            "--close-marker".to_owned(),
            path_argument(&close_marker),
        ],
    );

    let result = timeout(
        OUTER_TEST_TIMEOUT,
        run_managed_restart_probe(
            target,
            temporary_directory.path().to_path_buf(),
            Some("fixture_auth".to_owned()),
            PROBE_DEADLINE,
        ),
    )
    .await
    .expect("managed restart fault probe must not hang the test process");

    match result.expect_err("the synthetic recovery fault should fail the probe") {
        ProbeError::ManagedRestart(actual) => assert_eq!(actual, expected_error),
        other => panic!("expected a managed restart error, got {other}"),
    }

    assert_eq!(
        fs::read(&state_file).expect("the first process should persist its fixed state marker"),
        SESSION_STATE_MARKER,
        "the fixture must not persist a session identifier or request payload"
    );
    assert_eq!(
        fs::read(&close_marker).expect("the cleanup close request should write its marker"),
        SESSION_CLOSE_MARKER,
        "the cleanup marker must remain fixed and payload-free"
    );
}

fn path_argument(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
