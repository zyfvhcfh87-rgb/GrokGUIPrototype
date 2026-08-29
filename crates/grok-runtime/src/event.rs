use std::{fmt, path::PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use thiserror::Error;

use crate::RedactedDiagnostic;

/// Application-facing events produced by a Grok runtime adapter.
///
/// These are normalized presentation facts, not ACP envelopes. In particular,
/// an unknown extension retains only a fixed observation marker; its untrusted
/// raw method name and payload are never represented here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    RuntimeStateChanged {
        state: RuntimeState,
    },
    SessionStateChanged {
        session_id: String,
        state: SessionState,
    },
    MessageChunkReceived {
        session_id: String,
        message_id: Option<String>,
        text: String,
    },
    ThoughtChunkReceived {
        session_id: String,
        thought_id: Option<String>,
        text: String,
    },
    ToolCallChanged {
        session_id: String,
        call_id: String,
        title: String,
        kind: ToolCallKind,
        status: ActivityStatus,
        detail: Option<RedactedDiagnostic>,
    },
    PermissionRequested {
        session_id: String,
        request_id: String,
        title: String,
        consequence: Option<String>,
        kind: PermissionKind,
        can_persist_decision: bool,
    },
    ElicitationRequested {
        session_id: String,
        request_id: String,
        prompt: String,
        kind: ElicitationKind,
    },
    PlanChanged {
        session_id: String,
        entries: Vec<PlanEntry>,
    },
    UsageChanged {
        session_id: String,
        usage: Usage,
    },
    RuntimeFailed {
        diagnostic: RedactedDiagnostic,
        recoverable: bool,
    },
    ExtensionMethodObserved {
        session_id: Option<String>,
        method: ExtensionMethod,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Connecting,
    Authenticating,
    Ready,
    Working,
    WaitingForInput,
    Failed,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Creating,
    Ready,
    Working,
    WaitingForInput,
    Cancelling,
    Completed,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallKind {
    Tool,
    TerminalCommand,
    BackgroundTask,
    Subagent,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Pending,
    Running,
    WaitingForInput,
    Completed,
    Failed,
    Cancelled,
}

/// Exact permission scope shown to the user. Unknown request shapes collapse
/// to `Other` rather than retaining their raw payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionKind {
    Tool {
        tool_name: String,
    },
    Command {
        command: String,
        working_directory: Option<PathBuf>,
    },
    Filesystem {
        operation: String,
        path: PathBuf,
    },
    Network {
        destination: String,
    },
    Other,
}

/// Interaction shape for a normalized elicitation request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ElicitationKind {
    Text {
        placeholder: Option<String>,
        sensitive: bool,
    },
    Confirmation,
    Choice {
        options: Vec<String>,
        multiple: bool,
    },
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: PlanEntryStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEntryStatus {
    Pending,
    InProgress,
    Completed,
}

/// Usage supplied by the runtime. Missing values remain unknown rather than
/// being inferred from text or private files.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
}

/// A bounded JSON-RPC/ACP extension method name.
///
/// Restricting this to method-name characters is a defense-in-depth boundary.
/// The Grok adapter additionally maps unknown raw method names to a fixed marker
/// before constructing a normalized event.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ExtensionMethod(String);

impl ExtensionMethod {
    pub const MAX_LEN: usize = 256;

    pub fn new(method: impl Into<String>) -> Result<Self, InvalidExtensionMethod> {
        let method = method.into();

        if method.is_empty() {
            return Err(InvalidExtensionMethod::Empty);
        }

        if method.len() > Self::MAX_LEN {
            return Err(InvalidExtensionMethod::TooLong {
                length: method.len(),
                maximum: Self::MAX_LEN,
            });
        }

        if let Some(character) = method.chars().find(|character| {
            !(character.is_ascii_alphanumeric()
                || matches!(character, '.' | '_' | '-' | '/' | ':' | '$'))
        }) {
            return Err(InvalidExtensionMethod::InvalidCharacter(character));
        }

        Ok(Self(method))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for ExtensionMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for ExtensionMethod {
    type Error = InvalidExtensionMethod;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for ExtensionMethod {
    type Error = InvalidExtensionMethod;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for ExtensionMethod {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ExtensionMethod {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let method = String::deserialize(deserializer).map_err(D::Error::custom)?;
        Self::new(method).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum InvalidExtensionMethod {
    #[error("extension method name cannot be empty")]
    Empty,
    #[error("extension method name is {length} bytes; maximum is {maximum}")]
    TooLong { length: usize, maximum: usize },
    #[error("extension method name contains unsupported character `{0}`")]
    InvalidCharacter(char),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_events_round_trip_through_json() {
        let events = vec![
            RuntimeEvent::RuntimeStateChanged {
                state: RuntimeState::Ready,
            },
            RuntimeEvent::SessionStateChanged {
                session_id: "session-1".into(),
                state: SessionState::Working,
            },
            RuntimeEvent::MessageChunkReceived {
                session_id: "session-1".into(),
                message_id: Some("message-1".into()),
                text: "hello".into(),
            },
            RuntimeEvent::ThoughtChunkReceived {
                session_id: "session-1".into(),
                thought_id: None,
                text: "checking".into(),
            },
            RuntimeEvent::ToolCallChanged {
                session_id: "session-1".into(),
                call_id: "call-1".into(),
                title: "Inspect workspace".into(),
                kind: ToolCallKind::Tool,
                status: ActivityStatus::Running,
                detail: Some(RedactedDiagnostic::new("Bearer tool-secret")),
            },
            RuntimeEvent::PermissionRequested {
                session_id: "session-1".into(),
                request_id: "permission-1".into(),
                title: "Run tests".into(),
                consequence: Some("May create build artifacts".into()),
                kind: PermissionKind::Command {
                    command: "cargo test".into(),
                    working_directory: Some(PathBuf::from("workspace")),
                },
                can_persist_decision: false,
            },
            RuntimeEvent::ElicitationRequested {
                session_id: "session-1".into(),
                request_id: "elicitation-1".into(),
                prompt: "Pick one".into(),
                kind: ElicitationKind::Choice {
                    options: vec!["A".into(), "B".into()],
                    multiple: false,
                },
            },
            RuntimeEvent::PlanChanged {
                session_id: "session-1".into(),
                entries: vec![PlanEntry {
                    id: "step-1".into(),
                    title: "Probe runtime".into(),
                    description: None,
                    status: PlanEntryStatus::InProgress,
                }],
            },
            RuntimeEvent::UsageChanged {
                session_id: "session-1".into(),
                usage: Usage {
                    total_tokens: Some(42),
                    ..Usage::default()
                },
            },
            RuntimeEvent::RuntimeFailed {
                diagnostic: RedactedDiagnostic::new("api_key=runtime-secret"),
                recoverable: true,
            },
        ];

        for event in events {
            let encoded = serde_json::to_string(&event).unwrap();
            let decoded: RuntimeEvent = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, event);
            assert!(!encoded.contains("tool-secret"));
            assert!(!encoded.contains("runtime-secret"));
        }
    }

    #[test]
    fn unknown_extensions_retain_only_a_valid_method_name() {
        let event = RuntimeEvent::ExtensionMethodObserved {
            session_id: Some("session-1".into()),
            method: ExtensionMethod::new("x.ai/future_update").unwrap(),
        };

        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["method"], "x.ai/future_update");
        assert!(value.get("payload").is_none());
        assert!(ExtensionMethod::new(r#"x.ai/future {\"api_key\":\"secret\"}"#).is_err());
    }

    #[test]
    fn invalid_extension_method_is_rejected_during_deserialization() {
        let encoded = r#"{
            "type":"extension_method_observed",
            "session_id":null,
            "method":"x.ai/future method"
        }"#;

        let error = serde_json::from_str::<RuntimeEvent>(encoded).unwrap_err();

        assert!(error.to_string().contains("unsupported character"));
    }
}
