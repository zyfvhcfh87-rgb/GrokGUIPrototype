//! Stable application-facing types for talking to a Grok Build runtime.
//!
//! This crate owns the narrow seam between the desktop application and Grok,
//! including safe process management, ACP transport, and normalized events.
//! Platform-specific launch mechanics remain isolated behind their adapters.

mod diagnostic;
mod event;
mod executable;
mod grok_extension;
mod grok_session_response;
#[cfg(windows)]
mod windows_process;
mod wire_summary;

pub use diagnostic::{RedactedDiagnostic, redact_diagnostic};
pub use event::{
    ActivityStatus, ElicitationKind, ExtensionMethod, InvalidExtensionMethod, PermissionKind,
    PlanEntry, PlanEntryStatus, RuntimeEvent, RuntimeState, SessionState, ToolCallKind, Usage,
};
pub use executable::{
    GROK_PATH_ENV, GROK_STDIO_ARGS, GrokExecutableSource, ResolveGrokExecutableError,
    ResolvedGrokExecutable, resolve_grok_executable,
};
pub use grok_extension::{
    AccessGate, Announcement, AnnouncementBatch, AnnouncementCallToAction, GrokExtensionOutcome,
    GrokSettings, KnownExtensionMalformed, McpServerSource, McpServerStatus, McpServerStatusReason,
    McpServerSummary, McpUpdate, MetadataKeys, ModelCatalog, ModelDescriptor, OptionalSetting,
    PermissionMode, PromptCompletion, PromptQueueState, PromptStopReason, QueueEntryKind,
    QueuedPrompt, ReasoningEffortOption, SessionActivity, SessionChanges, SessionExtensionUpdate,
    SessionStatus, SessionStatusTrigger, SessionSummary, SessionUpdateKind, SessionUpdateRail,
    SessionUsage, normalize_grok_extension,
};
pub use grok_session_response::{
    GrokSessionConfigOption, GrokSessionResponse, SafeResponseKeys, normalize_grok_session_response,
};
#[cfg(windows)]
pub use windows_process::{ProcessDiagnosticSnapshot, ProcessDiagnostics, WindowsAcpProcess};
pub use wire_summary::{
    SanitizedWireFrame, WireCapture, WireDirection, WireFrameKind, WireSummary,
};
