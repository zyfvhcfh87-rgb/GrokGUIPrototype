use std::{
    ffi::{OsStr, OsString},
    fmt, io,
    path::PathBuf,
    pin::Pin,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex, MutexGuard},
    task::{Context, Poll},
    time::Duration,
};

use agent_client_protocol::{ConnectTo, LineDirection, Lines, Role};
use futures::{SinkExt, StreamExt};
use process_wrap::tokio::{ChildWrapper, CommandWrap, CreationFlags, JobObject, KillOnDrop};
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec, LinesCodecError};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::{WireCapture, wire_summary::MAX_WIRE_LINE_BYTES};

const MAX_STDERR_LINE_BYTES: usize = 2 * 1024;
const STDERR_SHUTDOWN_GRACE: Duration = Duration::from_millis(500);
const PROTOCOL_SHUTDOWN_GRACE: Duration = Duration::from_millis(500);
const CHILD_SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// A Windows ACP subprocess component contained in a kill-on-close Job Object.
///
/// The child is created suspended by `process-wrap`, assigned to the Job Object,
/// and only then resumed. Containment setup failures terminate the child and
/// fail the connection before any uncontained process can execute.
pub struct WindowsAcpProcess {
    executable: PathBuf,
    args: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    current_dir: Option<PathBuf>,
    diagnostics: ProcessDiagnostics,
    wire_capture: Option<WireCapture>,
}

impl WindowsAcpProcess {
    /// Build a managed ACP component for an already resolved executable.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            environment: Vec::new(),
            current_dir: None,
            diagnostics: ProcessDiagnostics::default(),
            wire_capture: None,
        }
    }

    /// Override one child environment variable while inheriting the rest of
    /// the ambient environment. The child is still spawned directly.
    #[must_use]
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.environment
            .push((key.as_ref().to_os_string(), value.as_ref().to_os_string()));
        self
    }

    /// Override child environment variables while inheriting unspecified
    /// ambient values. No shell expansion or interpolation is performed.
    #[must_use]
    pub fn envs<I, K, V>(mut self, variables: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.environment.extend(
            variables
                .into_iter()
                .map(|(key, value)| (key.as_ref().to_os_string(), value.as_ref().to_os_string())),
        );
        self
    }

    /// Append one argument without invoking a shell.
    #[must_use]
    pub fn arg(mut self, argument: impl AsRef<OsStr>) -> Self {
        self.args.push(argument.as_ref().to_os_string());
        self
    }

    /// Append arguments without invoking a shell.
    #[must_use]
    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_os_string()),
        );
        self
    }

    /// Set the already validated working directory for the child.
    #[must_use]
    pub fn current_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(directory.into());
        self
    }

    /// Return a cloneable handle to counters-only stderr diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> ProcessDiagnostics {
        self.diagnostics.clone()
    }

    /// Observe sanitized ACP frame shapes and diagnostic byte counts.
    #[must_use]
    pub fn wire_capture(mut self, capture: WireCapture) -> Self {
        self.wire_capture = Some(capture);
        self
    }

    fn spawn(self) -> Result<SpawnedProcess, agent_client_protocol::Error> {
        let Self {
            executable,
            args,
            environment,
            current_dir,
            diagnostics,
            wire_capture,
        } = self;
        let mut command = CommandWrap::with_new(executable, |command| {
            command
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command.envs(environment);
            if let Some(directory) = current_dir {
                command.current_dir(directory);
            }
        });

        // Registration order does not affect process-wrap's suspended-before-
        // assignment policy, but keeping the intent together makes review easy.
        command
            .wrap(CreationFlags(CREATE_NO_WINDOW))
            .wrap(JobObject)
            .wrap(KillOnDrop);

        let mut child = command
            .spawn()
            .map_err(|error| acp_process_error("failed to start contained ACP process", &error))?;
        let stdin = child
            .stdin()
            .take()
            .ok_or_else(|| acp_pipe_error("stdin"))?;
        let stdout = child
            .stdout()
            .take()
            .ok_or_else(|| acp_pipe_error("stdout"))?;
        let stderr = child
            .stderr()
            .take()
            .ok_or_else(|| acp_pipe_error("stderr"))?;

        Ok(SpawnedProcess {
            child,
            stdin,
            stdout,
            stderr,
            diagnostics,
            wire_capture,
        })
    }
}

impl fmt::Debug for WindowsAcpProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsAcpProcess")
            .field("argument_count", &self.args.len())
            .field("environment_override_count", &self.environment.len())
            .field("has_current_dir", &self.current_dir.is_some())
            .finish_non_exhaustive()
    }
}

impl<Counterpart: Role> ConnectTo<Counterpart> for WindowsAcpProcess {
    async fn connect_to(
        self,
        client: impl ConnectTo<Counterpart::Counterpart>,
    ) -> Result<(), agent_client_protocol::Error> {
        let SpawnedProcess {
            mut child,
            stdin,
            stdout,
            stderr,
            diagnostics,
            wire_capture,
        } = self.spawn()?;
        let mut stderr_task = StderrDrainTask::spawn(stderr, diagnostics, wire_capture.clone());
        let (stdin, stdin_control) = ControllableChildStdin::new(stdin);
        let transport = line_transport(stdin, stdout, wire_capture);

        let outcome = {
            let mut protocol = Box::pin(ConnectTo::<Counterpart>::connect_to(transport, client));
            let child_wait = child.wait();
            tokio::pin!(child_wait);

            tokio::select! {
                biased;
                result = &mut protocol => {
                    // Explicitly close the write pipe, then keep the Job Object
                    // alive while a well-behaved child and its descendants
                    // observe EOF and perform their own bounded cleanup.
                    drop(protocol);
                    stdin_control.close().await;
                    let child_status = match timeout(CHILD_SHUTDOWN_GRACE, &mut child_wait).await {
                        Ok(status) => Some(status.map_err(|error| {
                            acp_process_error("failed while waiting for ACP process", &error)
                        })?),
                        Err(_) => None,
                    };
                    ConnectionOutcome::Protocol(result, child_status)
                }
                status = &mut child_wait => {
                    let status = status.map_err(|error| {
                        acp_process_error("failed while waiting for ACP process", &error)
                    })?;
                    let protocol_result =
                        match timeout(PROTOCOL_SHUTDOWN_GRACE, &mut protocol).await {
                            Ok(result) => result,
                            Err(_) => Err(agent_client_protocol::Error::internal_error()
                                .data("ACP protocol did not close after its process exited")),
                    };
                    ConnectionOutcome::Child(status, protocol_result)
                }
            }
        };

        // This closes the Job Object. If graceful EOF cleanup timed out,
        // KillOnDrop is the forced fallback that terminates the entire tree,
        // including descendants which outlived the top-level child.
        drop(child);
        stderr_task.finish().await;

        match outcome {
            ConnectionOutcome::Protocol(Err(error), _) => Err(error),
            ConnectionOutcome::Protocol(Ok(()), Some(status)) if !status.success() => {
                Err(agent_client_protocol::Error::internal_error()
                    .data(format!("ACP process exited unsuccessfully ({status})")))
            }
            ConnectionOutcome::Protocol(result, _) => result,
            ConnectionOutcome::Child(status, protocol_result) if status.success() => {
                protocol_result
            }
            ConnectionOutcome::Child(status, _) => {
                Err(agent_client_protocol::Error::internal_error()
                    .data(format!("ACP process exited unsuccessfully ({status})")))
            }
        }
    }
}

struct SpawnedProcess {
    child: Box<dyn ChildWrapper>,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    diagnostics: ProcessDiagnostics,
    wire_capture: Option<WireCapture>,
}

fn line_transport(
    stdin: ControllableChildStdin,
    stdout: tokio::process::ChildStdout,
    wire_capture: Option<WireCapture>,
) -> Lines<
    impl futures::Sink<String, Error = io::Error> + Send + 'static,
    impl futures::Stream<Item = io::Result<String>> + Send + 'static,
> {
    let outgoing_capture = wire_capture.clone();
    let outgoing = FramedWrite::new(stdin, LinesCodec::new_with_max_length(MAX_WIRE_LINE_BYTES))
        .with(move |line: String| {
            let capture = outgoing_capture.clone();
            async move {
                if line.len() > MAX_WIRE_LINE_BYTES {
                    if let Some(capture) = &capture {
                        capture.record_malformed_transport_line(LineDirection::Stdin);
                    }
                    return Err(LinesCodecError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "outgoing ACP line exceeded the transport limit",
                    )));
                }
                if let Some(capture) = &capture {
                    capture.record_transport_line(&line, LineDirection::Stdin);
                }
                Ok(line)
            }
        })
        .sink_map_err(lines_codec_error);

    let incoming = FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_WIRE_LINE_BYTES))
        .map(move |result| match result {
            Ok(line) => {
                if let Some(capture) = &wire_capture {
                    capture.record_transport_line(&line, LineDirection::Stdout);
                }
                Ok(line)
            }
            Err(error) => {
                if is_malformed_line_error(&error)
                    && let Some(capture) = &wire_capture
                {
                    capture.record_malformed_transport_line(LineDirection::Stdout);
                }
                Err(lines_codec_error(error))
            }
        });

    Lines::new(outgoing, incoming)
}

fn is_malformed_line_error(error: &LinesCodecError) -> bool {
    match error {
        LinesCodecError::MaxLineLengthExceeded => true,
        LinesCodecError::Io(error) => error.kind() == io::ErrorKind::InvalidData,
    }
}

fn lines_codec_error(error: LinesCodecError) -> io::Error {
    match error {
        LinesCodecError::Io(error) => error,
        LinesCodecError::MaxLineLengthExceeded => io::Error::new(
            io::ErrorKind::InvalidData,
            "incoming ACP line exceeded the transport limit",
        ),
    }
}

enum ConnectionOutcome {
    Protocol(Result<(), agent_client_protocol::Error>, Option<ExitStatus>),
    Child(ExitStatus, Result<(), agent_client_protocol::Error>),
}

struct ControllableChildStdin {
    inner: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
}

impl ControllableChildStdin {
    fn new(stdin: tokio::process::ChildStdin) -> (Self, ChildStdinControl) {
        let inner = Arc::new(Mutex::new(Some(stdin)));
        (
            Self {
                inner: inner.clone(),
            },
            ChildStdinControl { inner },
        )
    }

    fn poll_with(
        &self,
        operation: impl FnOnce(Pin<&mut tokio::process::ChildStdin>) -> Poll<std::io::Result<usize>>,
    ) -> Poll<std::io::Result<usize>> {
        let mut inner = lock_unpoisoned(&self.inner);
        let Some(stdin) = inner.as_mut() else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "ACP child stdin is closed",
            )));
        };
        operation(Pin::new(stdin))
    }
}

impl AsyncWrite for ControllableChildStdin {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.poll_with(|stdin| stdin.poll_write(context, buffer))
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut inner = lock_unpoisoned(&self.inner);
        let Some(stdin) = inner.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        Pin::new(stdin).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut inner = lock_unpoisoned(&self.inner);
        let Some(stdin) = inner.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        Pin::new(stdin).poll_shutdown(context)
    }
}

struct ChildStdinControl {
    inner: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
}

impl ChildStdinControl {
    async fn close(&self) {
        let stdin = lock_unpoisoned(&self.inner).take();
        if let Some(mut stdin) = stdin {
            let _ = stdin.shutdown().await;
        }
    }
}

fn acp_pipe_error(pipe: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error()
        .data(format!("contained ACP process did not expose piped {pipe}"))
}

fn acp_process_error(
    context: &'static str,
    _error: &std::io::Error,
) -> agent_client_protocol::Error {
    // OS errors can echo executable and workspace paths outside the user's
    // home directory. The actionable, reviewed context is enough at this
    // persistence boundary; raw error text remains intentionally discarded.
    agent_client_protocol::Error::internal_error().data(context)
}

/// Cloneable access to counters-only child-process diagnostics.
#[derive(Clone, Default)]
pub struct ProcessDiagnostics(Arc<Mutex<DiagnosticState>>);

impl ProcessDiagnostics {
    /// Snapshot diagnostic counters without retaining child stderr content.
    #[must_use]
    pub fn snapshot(&self) -> ProcessDiagnosticSnapshot {
        let state = lock_unpoisoned(&self.0);
        ProcessDiagnosticSnapshot {
            total_bytes: state.total_bytes,
            total_lines: state.total_lines,
            truncated_lines: state.truncated_lines,
            read_errors: state.read_errors,
        }
    }

    fn record_line(&self, truncated: bool) {
        let mut state = lock_unpoisoned(&self.0);
        state.total_lines = state.total_lines.saturating_add(1);
        if truncated {
            state.truncated_lines = state.truncated_lines.saturating_add(1);
        }
    }

    fn record_bytes(&self, count: usize) {
        let mut state = lock_unpoisoned(&self.0);
        state.total_bytes = state.total_bytes.saturating_add(count as u64);
    }

    fn record_read_error(&self) {
        let mut state = lock_unpoisoned(&self.0);
        state.read_errors = state.read_errors.saturating_add(1);
    }
}

impl fmt::Debug for ProcessDiagnostics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.snapshot().fmt(formatter)
    }
}

/// Counters-only process diagnostics safe for display or persistence.
///
/// Child stderr contents are deliberately neither retained nor exposed because
/// arbitrary diagnostics can contain credentials and non-home filesystem paths.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessDiagnosticSnapshot {
    pub total_bytes: u64,
    pub total_lines: u64,
    pub truncated_lines: u64,
    pub read_errors: u64,
}

#[derive(Default)]
struct DiagnosticState {
    total_bytes: u64,
    total_lines: u64,
    truncated_lines: u64,
    read_errors: u64,
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct StderrDrainTask(Option<JoinHandle<()>>);

impl StderrDrainTask {
    fn spawn(
        stderr: tokio::process::ChildStderr,
        diagnostics: ProcessDiagnostics,
        wire_capture: Option<WireCapture>,
    ) -> Self {
        Self(Some(tokio::spawn(async move {
            drain_stderr(stderr, diagnostics, wire_capture).await;
        })))
    }

    async fn finish(&mut self) {
        let Some(mut task) = self.0.take() else {
            return;
        };
        if timeout(STDERR_SHUTDOWN_GRACE, &mut task).await.is_err() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for StderrDrainTask {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

async fn drain_stderr(
    mut stderr: impl AsyncRead + Unpin,
    diagnostics: ProcessDiagnostics,
    wire_capture: Option<WireCapture>,
) {
    let mut chunk = [0_u8; 1024];
    let mut line_bytes = 0_usize;
    let mut truncated = false;

    loop {
        let count = match stderr.read(&mut chunk).await {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => {
                diagnostics.record_read_error();
                return;
            }
        };
        diagnostics.record_bytes(count);

        for byte in &chunk[..count] {
            if *byte == b'\n' {
                diagnostics.record_line(truncated);
                if let Some(capture) = &wire_capture {
                    capture.record_diagnostic_line(line_bytes);
                }
                line_bytes = 0;
                truncated = false;
            } else {
                line_bytes = line_bytes.saturating_add(1);
                truncated |= line_bytes > MAX_STDERR_LINE_BYTES;
            }
        }
    }

    if line_bytes != 0 || truncated {
        diagnostics.record_line(truncated);
        if let Some(capture) = &wire_capture {
            capture.record_diagnostic_line(line_bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncWriteExt, duplex};

    use super::*;

    #[test]
    fn debug_output_never_exposes_environment_keys_or_values() {
        let process = WindowsAcpProcess::new("fixture-agent.exe")
            .env("XAI_API_KEY", "fixture-secret")
            .envs([("GROK_DISABLE_AUTO_UPDATE", "true")]);

        let debug = format!("{process:?}");

        assert!(debug.contains("environment_override_count: 2"));
        assert!(!debug.contains("XAI_API_KEY"));
        assert!(!debug.contains("fixture-secret"));
        assert!(!debug.contains("GROK_DISABLE_AUTO_UPDATE"));
    }

    #[test]
    fn process_errors_discard_raw_operating_system_text() {
        let raw =
            std::io::Error::other(r"Bearer private-token at E:\private-workspace\secret-agent.exe");

        let error = acp_process_error("failed to start contained ACP process", &raw);
        let debug = format!("{error:?}");

        assert!(debug.contains("failed to start contained ACP process"));
        assert!(!debug.contains("private-token"));
        assert!(!debug.contains("private-workspace"));
        assert!(!debug.contains("secret-agent"));
    }

    #[tokio::test]
    async fn stderr_drain_retains_only_safe_counters() {
        let (mut writer, reader) = duplex(16 * 1024);
        let diagnostics = ProcessDiagnostics::default();
        let captured = diagnostics.clone();
        let wire_capture = WireCapture::default();
        let drain = tokio::spawn(drain_stderr(reader, captured, Some(wire_capture.clone())));

        for index in 0..40 {
            writer
                .write_all(
                    format!("line {index} C:\\private\\workspace XAI_API_KEY=fixture-secret\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
        }
        writer
            .write_all(&vec![b'x'; MAX_STDERR_LINE_BYTES + 512])
            .await
            .unwrap();
        writer.write_all(b"\n").await.unwrap();
        drop(writer);
        drain.await.unwrap();

        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.total_lines, 41);
        assert_eq!(snapshot.truncated_lines, 1);
        assert_eq!(snapshot.read_errors, 0);

        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("fixture-secret"));
        assert!(!serialized.contains("XAI_API_KEY"));
        assert!(!serialized.contains("line 39"));
        assert!(!serialized.contains("private"));

        let wire_summary = wire_capture.summary();
        assert_eq!(wire_summary.stderr_line_count, 41);
        let serialized_wire = serde_json::to_string(&wire_summary).unwrap();
        assert!(!serialized_wire.contains("fixture-secret"));
        assert!(!serialized_wire.contains("XAI_API_KEY"));
        assert!(!serialized_wire.contains("private"));
    }
}
