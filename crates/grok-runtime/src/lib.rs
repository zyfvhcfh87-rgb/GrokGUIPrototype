//! Stable application-facing types for talking to a Grok Build runtime.
//!
//! Process management and ACP transport deliberately live outside this
//! foundation. This crate owns the narrow seam shared by those adapters and
//! the desktop application.

mod diagnostic;
mod event;
mod executable;

pub use diagnostic::{RedactedDiagnostic, redact_diagnostic};
pub use event::{
    ActivityStatus, ElicitationKind, ExtensionMethod, InvalidExtensionMethod, PermissionKind,
    PlanEntry, PlanEntryStatus, RuntimeEvent, RuntimeState, SessionState, ToolCallKind, Usage,
};
pub use executable::{
    GROK_PATH_ENV, GROK_STDIO_ARGS, GrokExecutableSource, ResolveGrokExecutableError,
    ResolvedGrokExecutable, resolve_grok_executable,
};
