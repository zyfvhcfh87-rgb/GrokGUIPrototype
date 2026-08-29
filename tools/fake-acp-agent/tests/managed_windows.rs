#![cfg(windows)]

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use agent_client_protocol::{Agent, ConnectionTo};
use agent_client_protocol::{Client, schema::v1::InitializeRequest};
use agent_client_protocol::{schema::ProtocolVersion, schema::v1::ClientCapabilities};
use grok_runtime::{WindowsAcpProcess, WireCapture};
use tokio::{sync::oneshot, time::timeout};
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
    System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    },
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn managed_acp_protocol_completion_allows_graceful_descendant_cleanup() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory should be created");
    let sentinel_directory = temporary_directory.path().join("graceful-sentinel");
    let (initialized_tx, initialized_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let wire_capture = WireCapture::default();
    let process = WindowsAcpProcess::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
        .args([
            "descendant".into(),
            "--sentinel-dir".into(),
            sentinel_directory.as_os_str().to_owned(),
        ])
        .wire_capture(wire_capture.clone());

    let connection = Client
        .builder()
        .name("managed-windows-graceful-exit-test")
        .connect_with(process, move |connection: ConnectionTo<Agent>| async move {
            let response = connection
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_capabilities(ClientCapabilities::new()),
                )
                .block_task()
                .await?;
            initialized_tx
                .send(response)
                .map_err(|_| agent_client_protocol::Error::internal_error())?;
            finish_rx
                .await
                .map_err(|_| agent_client_protocol::Error::internal_error())?;
            Ok(())
        });
    let task = tokio::spawn(connection);

    let initialized = timeout(STARTUP_TIMEOUT, initialized_rx)
        .await
        .expect("managed ACP process should initialize")
        .expect("initialize sender should remain alive");
    assert_eq!(initialized.protocol_version, ProtocolVersion::V1);

    let pid = wait_for_pid(&sentinel_directory.join("sentinel.ready")).await;
    let guard = ProcessGuard::open(pid).expect("test should open its own sentinel process");
    let before = wait_for_heartbeat(&sentinel_directory.join("sentinel.heartbeat"), 0).await;
    let after = wait_for_heartbeat(&sentinel_directory.join("sentinel.heartbeat"), before).await;
    assert!(after > before, "sentinel must be alive before graceful EOF");

    finish_tx
        .send(())
        .expect("test should release the connection callback");
    wait_for_contents(&sentinel_directory.join("sentinel.stop")).await;
    timeout(EXIT_TIMEOUT, task)
        .await
        .expect("managed connection should finish after protocol completion")
        .expect("managed connection task should not panic")
        .expect("managed connection should close successfully");

    let wire_summary = wire_capture.summary();
    assert!(wire_summary.client_methods.contains("initialize"));
    assert_eq!(wire_summary.agent_response_count, 1);

    wait_for_process_exit(guard.handle())
        .await
        .expect("graceful EOF cleanup must stop the descendant");
    assert_eq!(
        guard
            .exit_code()
            .expect("gracefully stopped sentinel exit code should be readable"),
        0,
        "graceful EOF must let the sentinel exit itself"
    );
    guard.disarm().expect("test process handle should close");
}

#[tokio::test]
async fn managed_acp_abrupt_abort_terminates_the_windows_job_and_its_descendant() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory should be created");
    let sentinel_directory = temporary_directory.path().join("managed-sentinel");
    let (initialized_tx, initialized_rx) = oneshot::channel();
    let process = WindowsAcpProcess::new(env!("CARGO_BIN_EXE_fake-acp-agent"))
        .args([
            "descendant".into(),
            "--sentinel-dir".into(),
            sentinel_directory.as_os_str().to_owned(),
        ])
        .arg("--linger-after-eof");

    let connection = Client
        .builder()
        .name("managed-windows-process-test")
        .connect_with(process, move |connection: ConnectionTo<Agent>| async move {
            let response = connection
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_capabilities(ClientCapabilities::new()),
                )
                .block_task()
                .await?;
            initialized_tx
                .send(response)
                .map_err(|_| agent_client_protocol::Error::internal_error())?;
            std::future::pending::<Result<(), agent_client_protocol::Error>>().await
        });
    let task = tokio::spawn(connection);

    let initialized = timeout(STARTUP_TIMEOUT, initialized_rx)
        .await
        .expect("managed ACP process should initialize")
        .expect("initialize sender should remain alive");
    assert_eq!(initialized.protocol_version, ProtocolVersion::V1);

    let pid = wait_for_pid(&sentinel_directory.join("sentinel.ready")).await;
    let guard = ProcessGuard::open(pid).expect("test should open its own sentinel process");
    let before = wait_for_heartbeat(&sentinel_directory.join("sentinel.heartbeat"), 0).await;
    let after = wait_for_heartbeat(&sentinel_directory.join("sentinel.heartbeat"), before).await;
    assert!(after > before, "sentinel must be alive before managed drop");

    task.abort();
    assert!(
        task.await
            .expect_err("aborted connection should be cancelled")
            .is_cancelled(),
        "connection task should report cancellation"
    );

    wait_for_process_exit(guard.handle())
        .await
        .expect("dropping the managed connection must terminate its descendant");
    assert!(
        !sentinel_directory.join("sentinel.stop").exists(),
        "abrupt abort must terminate the process tree before graceful EOF cleanup runs"
    );
    assert!(
        !sentinel_directory.join("sentinel.stopped").exists(),
        "abrupt Job termination must not be mistaken for a self-stopped sentinel"
    );
    guard.disarm().expect("test process handle should close");
}

async fn wait_for_pid(path: &Path) -> u32 {
    timeout(STARTUP_TIMEOUT, async {
        loop {
            if let Ok(contents) = fs::read_to_string(path)
                && let Ok(pid) = contents.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {}", path.display()))
}

async fn wait_for_heartbeat(path: &Path, minimum: u64) -> u64 {
    timeout(STARTUP_TIMEOUT, async {
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
    .unwrap_or_else(|_| panic!("timed out waiting for {} to advance", path.display()))
}

async fn wait_for_contents(path: &Path) -> String {
    timeout(STARTUP_TIMEOUT, async {
        loop {
            if let Ok(contents) = fs::read_to_string(path) {
                return contents;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {}", path.display()))
}

async fn wait_for_process_exit(handle: HANDLE) -> std::io::Result<()> {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        let wait = unsafe { WaitForSingleObject(handle, 0) };
        if wait == WAIT_OBJECT_0 {
            return Ok(());
        }
        if wait == WAIT_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "managed ACP descendant survived Job Object drop",
            ));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct ProcessGuard(Option<HANDLE>);

impl ProcessGuard {
    fn open(pid: u32) -> std::io::Result<Self> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
                false,
                pid,
            )
        }
        .map_err(std::io::Error::other)?;
        Ok(Self(Some(handle)))
    }

    fn handle(&self) -> HANDLE {
        self.0.expect("only disarm consumes the process handle")
    }

    fn exit_code(&self) -> std::io::Result<u32> {
        let mut exit_code = 0;
        unsafe { GetExitCodeProcess(self.handle(), &mut exit_code) }
            .map_err(std::io::Error::other)?;
        Ok(exit_code)
    }

    fn disarm(mut self) -> std::io::Result<()> {
        if let Some(handle) = self.0.take() {
            unsafe { CloseHandle(handle) }.map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            unsafe { TerminateProcess(handle, 1) }.ok();
            let _ = unsafe { WaitForSingleObject(handle, 2_000) };
            unsafe { CloseHandle(handle) }.ok();
        }
    }
}
