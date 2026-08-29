//! Normalization for Grok Build's private `x.ai/*` ACP notifications.
//!
//! Raw extension JSON terminates at this module. Callers get typed snapshots,
//! bounded key summaries, or a method-name-only [`RuntimeEvent`] for an
//! extension this version does not understand.

use std::{collections::HashSet, fmt, marker::PhantomData};

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeOwned, IgnoredAny, SeqAccess, Visitor},
};
use serde_json::Value;

use crate::{ExtensionMethod, InvalidExtensionMethod, RuntimeEvent, redact_diagnostic};

const WIRE_PREFIX: &str = "_x.ai/";
const MAX_SUMMARY_KEYS: usize = 32;
pub(crate) const MAX_PROTOCOL_TOKEN_LEN: usize = 64;
pub(crate) const MAX_IDENTIFIER_BYTES: usize = 256;
pub(crate) const MAX_DISPLAY_TEXT_BYTES: usize = 512;
pub(crate) const MAX_DESCRIPTION_BYTES: usize = 4 * 1_024;
const MAX_MESSAGE_BYTES: usize = 8 * 1_024;
const MAX_URL_BYTES: usize = 2 * 1_024;
pub(crate) const MAX_MODELS: usize = 128;
pub(crate) const MAX_REASONING_EFFORTS: usize = 32;
const MAX_SESSION_CHANGES: usize = 512;
const MAX_QUEUE_ENTRIES: usize = 256;
const MAX_MCP_SERVERS: usize = 256;
const MAX_ANNOUNCEMENTS: usize = 64;
const UNKNOWN_EXTENSION_METHOD: &str = "x.ai/unknown";
const UNKNOWN_METADATA_KEY: &str = "<unknown>";
const TRUNCATED_METADATA_KEY: &str = "<truncated>";

#[derive(Debug)]
struct BoundedVec<T, const MAX: usize> {
    values: Vec<T>,
    overflowed: bool,
}

impl<T, const MAX: usize> Default for BoundedVec<T, MAX> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            overflowed: false,
        }
    }
}

impl<T, const MAX: usize> BoundedVec<T, MAX> {
    fn into_complete(self) -> Option<Vec<T>> {
        (!self.overflowed).then_some(self.values)
    }
}

impl<'de, T, const MAX: usize> Deserialize<'de> for BoundedVec<T, MAX>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BoundedVecVisitor<T, const MAX: usize>(PhantomData<T>);

        impl<'de, T, const MAX: usize> Visitor<'de> for BoundedVecVisitor<T, MAX>
        where
            T: Deserialize<'de>,
        {
            type Value = BoundedVec<T, MAX>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a sequence retaining at most {MAX} items")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let capacity = sequence.size_hint().unwrap_or_default().min(MAX);
                let mut values = Vec::with_capacity(capacity);

                while values.len() < MAX {
                    let Some(value) = sequence.next_element()? else {
                        return Ok(BoundedVec {
                            values,
                            overflowed: false,
                        });
                    };
                    values.push(value);
                }

                let overflowed = sequence.next_element::<IgnoredAny>()?.is_some();
                if overflowed {
                    while sequence.next_element::<IgnoredAny>()?.is_some() {}
                }
                Ok(BoundedVec { values, overflowed })
            }
        }

        deserializer.deserialize_seq(BoundedVecVisitor(PhantomData))
    }
}

fn bounded_string(value: String, maximum_bytes: usize) -> String {
    bounded_str(&value, maximum_bytes)
}

fn bounded_optional_string(value: Option<String>, maximum_bytes: usize) -> Option<String> {
    value.map(|value| bounded_string(value, maximum_bytes))
}

fn bounded_str(value: &str, maximum_bytes: usize) -> String {
    let sanitized = redact_diagnostic(value);
    bounded_plain_str(&sanitized, maximum_bytes)
}

fn bounded_plain_str(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.to_owned();
    }

    let mut boundary = maximum_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

fn exact_bounded_string(value: String, maximum_bytes: usize) -> Option<String> {
    is_exact_actionable_value(&value, maximum_bytes).then(|| value.as_str().to_owned())
}

fn exact_optional_string(value: Option<String>, maximum_bytes: usize) -> Option<Option<String>> {
    match value {
        Some(value) => exact_bounded_string(value, maximum_bytes).map(Some),
        None => Some(None),
    }
}

fn exact_bounded_str(value: &str, maximum_bytes: usize) -> Option<String> {
    is_exact_actionable_value(value, maximum_bytes).then(|| value.to_owned())
}

fn exact_optional_str(value: Option<&str>, maximum_bytes: usize) -> Option<Option<String>> {
    match value {
        Some(value) => exact_bounded_str(value, maximum_bytes).map(Some),
        None => Some(None),
    }
}

fn is_exact_actionable_value(value: &str, maximum_bytes: usize) -> bool {
    is_exact_actionable_syntax(value, maximum_bytes) && redact_diagnostic(value) == value
}

fn is_exact_actionable_syntax(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= maximum_bytes && !value.chars().any(char::is_control)
}

fn exact_optional_url(value: Option<String>) -> Option<Option<String>> {
    match value {
        Some(value) => exact_bounded_url(value).map(Some),
        None => Some(None),
    }
}

fn exact_bounded_url(value: String) -> Option<String> {
    (is_exact_actionable_syntax(&value, MAX_URL_BYTES) && is_safe_external_url(&value))
        .then(|| value.as_str().to_owned())
}

/// Keep actionable links deliberately conservative at the raw-extension seam.
/// Query strings, fragments, userinfo, escapes, and non-HTTP schemes can all
/// carry credential material or conceal a local path, so callers receive no
/// URL rather than an ambiguous one.
fn is_safe_external_url(value: &str) -> bool {
    let Some(remainder) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    let (authority, path) = remainder.split_once('/').unwrap_or((remainder, ""));
    if !is_safe_url_authority(authority) {
        return false;
    }

    redact_diagnostic(authority) == authority && is_safe_url_path(path)
}

fn is_safe_url_path(path: &str) -> bool {
    if !path.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~')
    }) {
        return false;
    }

    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let first = segments.next();
    if first.is_some_and(is_private_filesystem_root) {
        return false;
    }
    first.into_iter().chain(segments).all(|segment| {
        segment.len() <= MAX_PROTOCOL_TOKEN_LEN
            && !matches!(segment, "." | "..")
            && !is_credential_label(segment)
            && redact_diagnostic(segment) == segment
    })
}

fn is_private_filesystem_root(segment: &str) -> bool {
    ["Users", "home", "root", "srv", "var", "tmp", "private"]
        .iter()
        .any(|candidate| segment.eq_ignore_ascii_case(candidate))
}

fn is_credential_label(segment: &str) -> bool {
    [
        "token",
        "access_token",
        "access-token",
        "refresh_token",
        "refresh-token",
        "auth",
        "auth_token",
        "auth-token",
        "authorization",
        "bearer",
        "secret",
        "client_secret",
        "client-secret",
        "api_key",
        "api-key",
        "apikey",
        "key",
    ]
    .iter()
    .any(|candidate| segment.eq_ignore_ascii_case(candidate))
}

fn is_safe_url_authority(authority: &str) -> bool {
    if authority.is_empty() || authority.contains('@') {
        return false;
    }

    if let Some(address) = authority.strip_prefix('[') {
        let Some((address, suffix)) = address.split_once(']') else {
            return false;
        };
        return !address.is_empty()
            && address
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'.'))
            && valid_optional_port(suffix);
    }

    let mut parts = authority.split(':');
    let host = parts.next().unwrap_or_default();
    let port = parts.next();
    if parts.next().is_some()
        || host.is_empty()
        || host.starts_with('.')
        || host.starts_with('-')
        || host.ends_with('.')
        || host.ends_with('-')
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return false;
    }
    port.is_none_or(valid_port)
}

fn valid_optional_port(suffix: &str) -> bool {
    suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_port)
}

fn valid_port(port: &str) -> bool {
    !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}

/// Normalize one Grok extension notification.
///
/// Both the raw wire spelling (`_x.ai/*`) and the official SDK spelling
/// (`x.ai/*`) are accepted. Dispatch is otherwise exact and case-sensitive.
/// Unknown methods discard both `payload` and their untrusted raw method name.
pub fn normalize_grok_extension(
    method: &str,
    payload: Value,
) -> Result<GrokExtensionOutcome, InvalidExtensionMethod> {
    let method = ExtensionMethod::new(method)?;
    let canonical = canonical_method(method.as_str()).to_owned();

    if !is_known_method(&canonical) {
        return Ok(GrokExtensionOutcome::RuntimeEvent(
            RuntimeEvent::ExtensionMethodObserved {
                session_id: None,
                method: ExtensionMethod::new(UNKNOWN_EXTENSION_METHOD)
                    .expect("fixed unknown-extension marker must remain valid"),
            },
        ));
    }

    let envelope_keys = MetadataKeys::from_value(&payload);
    let Some(payload) = unwrap_wire_payload(&canonical, payload) else {
        return Ok(GrokExtensionOutcome::Malformed(KnownExtensionMalformed {
            method,
            payload_keys: envelope_keys,
        }));
    };
    let payload_keys = MetadataKeys::from_value(&payload);
    let malformed = KnownExtensionMalformed {
        method,
        payload_keys: payload_keys.clone(),
    };

    let outcome = match canonical.as_str() {
        "x.ai/models/update" => decode::<ModelCatalogDto>(payload)
            .and_then(|dto| normalize_models(dto, payload_keys))
            .map(GrokExtensionOutcome::ModelsUpdated)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        "x.ai/settings/update" => decode::<SettingsDto>(payload)
            .and_then(|dto| normalize_settings(dto, payload_keys))
            .map(GrokExtensionOutcome::SettingsUpdated)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        "x.ai/sessions/changed" => decode::<SessionChangesDto>(payload)
            .and_then(normalize_sessions)
            .map(GrokExtensionOutcome::SessionsChanged)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        "x.ai/queue/changed" => decode::<QueueStateDto>(payload)
            .and_then(normalize_queue)
            .map(GrokExtensionOutcome::QueueChanged)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        "x.ai/session/prompt_complete" => decode::<PromptCompletionDto>(payload)
            .and_then(normalize_prompt_completion)
            .map(GrokExtensionOutcome::PromptCompleted)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        "x.ai/session_notification" => {
            normalize_session_update(payload, SessionUpdateRail::Notification, malformed)
        }
        "x.ai/session/update" => {
            normalize_session_update(payload, SessionUpdateRail::Persisted, malformed)
        }
        "x.ai/announcements/update" => decode::<AnnouncementBatchDto>(payload)
            .and_then(normalize_announcements)
            .map(GrokExtensionOutcome::AnnouncementsUpdated)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        "x.ai/mcp/init_progress" => decode::<McpInitProgressDto>(payload)
            .and_then(|dto| {
                if dto.connected > dto.total {
                    return None;
                }
                Some(McpUpdate::InitProgress {
                    session_id: exact_optional_string(dto.session_id, MAX_IDENTIFIER_BYTES)?,
                    total: dto.total,
                    connected: dto.connected,
                })
            })
            .map(GrokExtensionOutcome::McpChanged)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        "x.ai/mcp/server_status" => decode::<McpServerStatusDto>(payload)
            .and_then(normalize_mcp_server_status)
            .map(GrokExtensionOutcome::McpChanged)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        "x.ai/mcp/servers_updated" => decode::<McpServersUpdatedDto>(payload)
            .and_then(normalize_mcp_servers)
            .map(GrokExtensionOutcome::McpChanged)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        "x.ai/mcp_initialized" => decode::<McpInitializedDto>(payload)
            .and_then(|dto| {
                Some(McpUpdate::Initialized {
                    session_id: exact_bounded_string(dto.session_id, MAX_IDENTIFIER_BYTES)?,
                    tool_count: dto.mcp_tool_count,
                    elapsed_ms: dto.elapsed_ms,
                })
            })
            .map(GrokExtensionOutcome::McpChanged)
            .unwrap_or(GrokExtensionOutcome::Malformed(malformed)),
        _ => unreachable!("known-method guard and dispatch must stay in sync"),
    };

    Ok(outcome)
}

/// Application-facing result of normalizing one extension notification.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum GrokExtensionOutcome {
    ModelsUpdated(ModelCatalog),
    SettingsUpdated(GrokSettings),
    SessionsChanged(SessionChanges),
    QueueChanged(PromptQueueState),
    PromptCompleted(PromptCompletion),
    SessionUpdated(Box<SessionExtensionUpdate>),
    AnnouncementsUpdated(AnnouncementBatch),
    McpChanged(McpUpdate),
    Malformed(KnownExtensionMalformed),
    RuntimeEvent(RuntimeEvent),
}

/// A known method whose payload did not satisfy its minimum safe shape.
///
/// The parse error and values are deliberately absent because either could
/// echo untrusted or credential-bearing payload content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct KnownExtensionMalformed {
    pub method: ExtensionMethod,
    pub payload_keys: MetadataKeys,
}

/// Bounded allowlisted object-key names observed at a discarded JSON seam.
/// All unrecognized names collapse to one marker so attacker-controlled keys
/// can never become a data-smuggling channel. When the reviewed-key limit is
/// exceeded, the final entry is a deterministic truncation marker.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MetadataKeys(Vec<String>);

impl MetadataKeys {
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    fn from_value(value: &Value) -> Self {
        let Some(object) = value.as_object() else {
            return Self::default();
        };

        Self::from_object(object)
    }

    fn from_object(object: &serde_json::Map<String, Value>) -> Self {
        let mut keys = Vec::new();
        let mut has_unknown = false;

        for key in object.keys() {
            if is_known_metadata_key(key) {
                keys.push(key.clone());
            } else {
                has_unknown = true;
            }
        }

        if has_unknown {
            keys.push(UNKNOWN_METADATA_KEY.to_owned());
        }

        keys.sort_unstable();
        keys.dedup();
        if keys.len() > MAX_SUMMARY_KEYS {
            keys.truncate(MAX_SUMMARY_KEYS - 1);
            keys.push(TRUNCATED_METADATA_KEY.to_owned());
        }
        Self(keys)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelCatalog {
    pub current_model_id: String,
    pub available_models: Vec<ModelDescriptor>,
    pub payload_keys: MetadataKeys,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelDescriptor {
    pub model_id: String,
    pub name: String,
    pub description: Option<String>,
    pub agent_type: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_efforts: Vec<ReasoningEffortOption>,
    pub supports_reasoning_effort: Option<bool>,
    pub total_context_tokens: Option<u64>,
    pub metadata_keys: MetadataKeys,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReasoningEffortOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub value: String,
    pub is_default: bool,
}

/// Presence-aware update for a setting where omission and explicit `null`
/// have different meanings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum OptionalSetting<T> {
    NotReported,
    Cleared,
    Value(T),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GrokSettings {
    pub permission_mode: OptionalSetting<PermissionMode>,
    pub auto_permission_mode_enabled: Option<bool>,
    pub show_resolved_model: Option<bool>,
    pub voice_mode_enabled: Option<bool>,
    pub sharing_enabled: Option<bool>,
    pub privacy_notice_rollout: Option<bool>,
    pub privacy_banner_reshow_days: Option<u64>,
    pub session_picker_grouped: Option<bool>,
    pub group_tool_verbs: Option<bool>,
    pub collapsed_edit_blocks: Option<bool>,
    pub subscription_watch_interval_secs: Option<u64>,
    pub access_allowed: Option<bool>,
    pub access_gate: Option<AccessGate>,
    pub payload_keys: MetadataKeys,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Default,
    Ask,
    Auto,
    AlwaysApprove,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AccessGate {
    pub message: Option<String>,
    pub url: Option<String>,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionChanges {
    pub upserted: Vec<SessionSummary>,
    pub removed_session_ids: Vec<String>,
}

/// Privacy-minimized session roster row.
///
/// Grok's `cwd`, origin host, and last-turn summary are intentionally omitted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: Option<String>,
    pub is_worktree: Option<bool>,
    pub model_id: Option<String>,
    pub reasoning_effort: Option<String>,
    pub always_approve: Option<bool>,
    pub activity: SessionActivity,
    pub resident: Option<bool>,
    pub last_change_unix_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionActivity {
    Working,
    Idle,
    NeedsInput,
    Dormant,
    Completed,
    Dead,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PromptQueueState {
    pub session_id: String,
    pub entries: Vec<QueuedPrompt>,
    pub running_prompt_id: Option<String>,
}

/// Structural queue information only. Prompt text and client attribution stay
/// inside the adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueuedPrompt {
    pub id: String,
    pub version: u64,
    pub kind: QueueEntryKind,
    pub position: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueEntryKind {
    Prompt,
    Bash,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpUpdate {
    InitProgress {
        session_id: Option<String>,
        total: u64,
        connected: u64,
    },
    ServerStatus {
        session_id: String,
        name: String,
        source: McpServerSource,
        status: McpServerStatus,
        reason: McpServerStatusReason,
        /// Whether Grok supplied additional diagnostic context.
        ///
        /// The content is intentionally discarded at deserialization because it can contain
        /// arbitrary paths, credentials, or private server output.
        detail_present: bool,
        tool_count: Option<usize>,
    },
    ServersUpdated {
        servers: Vec<McpServerSummary>,
    },
    Initialized {
        session_id: String,
        tool_count: Option<u64>,
        elapsed_ms: Option<u64>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct McpServerSummary {
    pub name: String,
    pub display_name: Option<String>,
    pub source: McpServerSource,
    pub enabled: Option<bool>,
    pub status: Option<McpServerStatus>,
    pub tool_count: Option<usize>,
    pub auth_required: Option<bool>,
    pub setup_required: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerSource {
    Managed,
    Local,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerStatus {
    Ready,
    Initializing,
    SetupRequired,
    Unavailable,
    NeedsAuth,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerStatusReason {
    TransportClosed,
    HandshakeFailed,
    ConfigAdded,
    ConfigRemoved,
    ConfigChanged,
    Disabled,
    AuthExpired,
    Initialized,
    RestartSucceeded,
    RestartFailed,
    ManagedTokenRefreshed,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PromptCompletion {
    pub session_id: String,
    pub prompt_id: String,
    pub stop_reason: PromptStopReason,
    pub error_kind: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptStopReason {
    EndTurn,
    ToolUse,
    StopSequence,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Error,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionExtensionUpdate {
    pub session_id: String,
    pub rail: SessionUpdateRail,
    pub kind: SessionUpdateKind,
    pub prompt_id: Option<String>,
    pub stop_reason: Option<PromptStopReason>,
    pub error_kind: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub model_id: Option<String>,
    pub reasoning_effort: Option<String>,
    pub usage: Option<SessionUsage>,
    pub status: Option<SessionStatus>,
    pub replay: bool,
    pub metadata_keys: MetadataKeys,
    pub update_keys: MetadataKeys,
}

/// Token and timing counters supplied by Grok for either a completed turn or
/// one model response. Costs and per-model billing rows are intentionally not
/// projected through this boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SessionUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub model_calls: Option<u64>,
    pub api_duration_ms: Option<u64>,
    pub turn_count: Option<u64>,
    pub incomplete: Option<bool>,
}

/// Safe subset of the `session_status` snapshot. Workspace, transcript, repo,
/// worktree, and billing fields are deliberately excluded because they can
/// contain private paths or account information.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionStatus {
    pub schema_version: Option<u64>,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub reasoning_effort: Option<String>,
    pub context_window_tokens: Option<u64>,
    pub context_tokens: Option<u64>,
    pub usage: Option<SessionUsage>,
    pub used_percentage: Option<u8>,
    pub remaining_percentage: Option<u8>,
    pub auto_compact_threshold_percentage: Option<u8>,
    pub turn_started_at_ms: Option<i64>,
    pub trigger: Option<SessionStatusTrigger>,
    pub payload_keys: MetadataKeys,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatusTrigger {
    State,
    RefreshInterval,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionUpdateRail {
    Notification,
    Persisted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionUpdateKind {
    TurnCompleted,
    ResponseStarted,
    ResponseCompleted,
    ReasoningCompleted,
    ModelChanged,
    SessionStatus,
    PendingInteraction,
    InteractionResolved,
    SessionSummaryGenerated,
    ToolCallDeltaChunk,
    HookExecution,
    SubagentSpawned,
    SubagentFinished,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AnnouncementBatch {
    pub generation: u64,
    pub announcements: Vec<Announcement>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Announcement {
    pub id: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
    pub severity: Option<String>,
    pub dismissible: Option<bool>,
    pub persistent: Option<bool>,
    pub call_to_action: Option<AnnouncementCallToAction>,
}

/// Callers must validate this URL before opening it outside the application.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AnnouncementCallToAction {
    pub label: Option<String>,
    pub url: Option<String>,
    pub caption: Option<String>,
}

fn canonical_method(method: &str) -> &str {
    if method.starts_with(WIRE_PREFIX) {
        &method[1..]
    } else {
        method
    }
}

fn is_known_method(method: &str) -> bool {
    matches!(
        method,
        "x.ai/announcements/update"
            | "x.ai/mcp/init_progress"
            | "x.ai/mcp/server_status"
            | "x.ai/mcp/servers_updated"
            | "x.ai/mcp_initialized"
            | "x.ai/models/update"
            | "x.ai/queue/changed"
            | "x.ai/session/prompt_complete"
            | "x.ai/session/update"
            | "x.ai/session_notification"
            | "x.ai/sessions/changed"
            | "x.ai/settings/update"
    )
}

fn unwrap_wire_payload(method: &str, payload: Value) -> Option<Value> {
    let Value::Object(mut object) = payload else {
        return Some(payload);
    };

    let looks_like_envelope = ["jsonrpc", "method", "params", "id", "result", "error"]
        .iter()
        .any(|key| object.contains_key(*key));
    if !looks_like_envelope {
        return Some(Value::Object(object));
    }

    let valid_notification = object.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && object
            .get("method")
            .and_then(Value::as_str)
            .map(canonical_method)
            == Some(method)
        && object.get("params").is_some_and(Value::is_object)
        && !["id", "result", "error"]
            .iter()
            .any(|key| object.contains_key(*key));
    valid_notification.then(|| {
        object
            .remove("params")
            .expect("validated notification params must remain present")
    })
}

fn decode<T: DeserializeOwned>(payload: Value) -> Option<T> {
    payload
        .is_object()
        .then(|| serde_json::from_value(payload).ok())
        .flatten()
}

fn is_known_metadata_key(key: &str) -> bool {
    matches!(
        key,
        // Shared envelopes and correlation.
        "_meta"
            | "jsonrpc"
            | "method"
            | "params"
            | "result"
            | "error"
            | "sessionId"
            | "session_id"
            | "promptId"
            | "prompt_id"
            | "update"
            | "isReplay"
            | "eventId"
            // Models and negotiated reasoning.
            | "currentModelId"
            | "availableModels"
            | "modelId"
            | "model_id"
            | "name"
            | "description"
            | "agentType"
            | "reasoningEffort"
            | "reasoning_effort"
            | "reasoningEfforts"
            | "supportsReasoningEffort"
            | "totalContextTokens"
            // Settings fields observed on the 1.0.5 notification.
            | "permission_mode"
            | "auto_permission_mode_enabled"
            | "show_resolved_model"
            | "voice_mode_enabled"
            | "sharing_enabled"
            | "privacy_notice_rollout"
            | "privacy_banner_reshow_days"
            | "session_picker_grouped"
            | "group_tool_verbs"
            | "collapsed_edit_blocks"
            | "subscription_watch_interval_secs"
            | "allow_access"
            | "gate_message"
            | "gate_url"
            | "gate_label"
            | "tips"
            | "slash_command_tags"
            | "announcements"
            | "campaigns"
            | "consent_gate"
            | "subscription_tier_display"
            // Session roster and queue structure. Content-bearing values are
            // discarded elsewhere; these literal field names are safe.
            | "upserted"
            | "removed"
            | "title"
            | "cwd"
            | "isWorktree"
            | "yolo"
            | "activity"
            | "resident"
            | "lastChangeUnixMs"
            | "origin"
            | "entries"
            | "runningPromptId"
            | "runningText"
            | "runningKind"
            | "runningCombinedTexts"
            | "id"
            | "version"
            | "owner"
            | "lastEditor"
            | "kind"
            | "text"
            | "combinedTexts"
            | "position"
            // Session update tags, terminal state, and usage.
            | "sessionUpdate"
            | "stopReason"
            | "stop_reason"
            | "errorKind"
            | "error_kind"
            | "elapsedMs"
            | "elapsed_ms"
            | "usage"
            | "inputTokens"
            | "input_tokens"
            | "outputTokens"
            | "output_tokens"
            | "totalTokens"
            | "total_tokens"
            | "cachedReadTokens"
            | "cached_read_tokens"
            | "cacheReadInputTokens"
            | "cache_read_input_tokens"
            | "cacheCreationTokens"
            | "cache_creation_tokens"
            | "cacheCreationInputTokens"
            | "cache_creation_input_tokens"
            | "reasoningTokens"
            | "reasoning_tokens"
            | "modelCalls"
            | "model_calls"
            | "apiDurationMs"
            | "api_duration_ms"
            | "numTurns"
            | "num_turns"
            | "usageIsIncomplete"
            | "usage_is_incomplete"
            // Privacy-minimized status snapshot fields.
            | "schemaVersion"
            | "schema_version"
            | "session_name"
            | "transcript_path"
            | "model"
            | "displayName"
            | "display_name"
            | "effort"
            | "level"
            | "workspace"
            | "cost"
            | "contextWindow"
            | "context_window"
            | "contextWindowSize"
            | "context_window_size"
            | "contextTokens"
            | "context_tokens"
            | "sessionInputTokens"
            | "session_input_tokens"
            | "sessionOutputTokens"
            | "session_output_tokens"
            | "sessionUsage"
            | "session_usage"
            | "usedPercentage"
            | "used_percentage"
            | "remainingPercentage"
            | "remaining_percentage"
            | "autoCompactThresholdPercent"
            | "auto_compact_threshold_percent"
            | "turn"
            | "startedAtMs"
            | "started_at_ms"
            | "trigger"
            | "worktree"
            // Announcements and MCP state.
            | "gen"
            | "severity"
            | "dismissible"
            | "persistent"
            | "cta"
            | "label"
            | "url"
            | "caption"
            | "total"
            | "connected"
            | "source"
            | "status"
            | "reason"
            | "detail"
            | "tools"
            | "mcpServers"
            | "mcpToolCount"
            | "enabled"
            | "authRequired"
            | "setupRequired"
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelCatalogDto {
    current_model_id: String,
    available_models: BoundedVec<ModelDto, MAX_MODELS>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelDto {
    model_id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "_meta", default)]
    metadata: Option<serde_json::Map<String, Value>>,
}

fn normalize_models(dto: ModelCatalogDto, payload_keys: MetadataKeys) -> Option<ModelCatalog> {
    let current_model_id = exact_bounded_string(dto.current_model_id, MAX_IDENTIFIER_BYTES)?;
    let available_models = dto
        .available_models
        .into_complete()?
        .into_iter()
        .map(normalize_model)
        .collect::<Option<Vec<_>>>()?;
    let mut model_ids = HashSet::with_capacity(available_models.len());
    if available_models
        .iter()
        .any(|model| !model_ids.insert(model.model_id.as_str()))
        || !model_ids.contains(current_model_id.as_str())
    {
        return None;
    }

    Some(ModelCatalog {
        current_model_id,
        available_models,
        payload_keys,
    })
}

fn normalize_model(dto: ModelDto) -> Option<ModelDescriptor> {
    let metadata_keys = dto
        .metadata
        .as_ref()
        .map_or_else(MetadataKeys::default, MetadataKeys::from_object);
    let metadata = dto.metadata.as_ref();
    let exact_string = |key: &str, maximum_bytes: usize| {
        let value = optional_str_field(metadata, key)?;
        exact_optional_str(value, maximum_bytes)
    };
    let reasoning_effort_values = optional_array_field(metadata, "reasoningEfforts")?;
    if reasoning_effort_values.is_some_and(|values| values.len() > MAX_REASONING_EFFORTS) {
        return None;
    }
    let reasoning_efforts = reasoning_effort_values
        .into_iter()
        .flatten()
        .map(|value| {
            let option = decode::<ReasoningEffortDto>(value.clone())?;
            Some(ReasoningEffortOption {
                id: exact_bounded_string(option.id, MAX_PROTOCOL_TOKEN_LEN)?,
                label: bounded_string(option.label, MAX_DISPLAY_TEXT_BYTES),
                description: bounded_optional_string(option.description, MAX_DESCRIPTION_BYTES),
                value: exact_bounded_string(option.value, MAX_PROTOCOL_TOKEN_LEN)?,
                is_default: option.is_default,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let reasoning_effort = exact_string("reasoningEffort", MAX_PROTOCOL_TOKEN_LEN)?;
    let mut effort_ids = HashSet::with_capacity(reasoning_efforts.len());
    let mut effort_values = HashSet::with_capacity(reasoning_efforts.len());
    if reasoning_efforts.iter().any(|effort| {
        !effort_ids.insert(effort.id.as_str()) || !effort_values.insert(effort.value.as_str())
    }) || reasoning_efforts
        .iter()
        .filter(|effort| effort.is_default)
        .take(2)
        .count()
        > 1
        || reasoning_effort
            .as_ref()
            .is_some_and(|selected| !effort_values.contains(selected.as_str()))
    {
        return None;
    }

    Some(ModelDescriptor {
        model_id: exact_bounded_string(dto.model_id, MAX_IDENTIFIER_BYTES)?,
        name: bounded_string(dto.name, MAX_DISPLAY_TEXT_BYTES),
        description: bounded_optional_string(dto.description, MAX_DESCRIPTION_BYTES),
        agent_type: exact_string("agentType", MAX_PROTOCOL_TOKEN_LEN)?,
        reasoning_effort,
        reasoning_efforts,
        supports_reasoning_effort: optional_bool_field(metadata, "supportsReasoningEffort")?,
        total_context_tokens: optional_u64_field(metadata, "totalContextTokens")?,
        metadata_keys,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReasoningEffortDto {
    id: String,
    label: String,
    #[serde(default)]
    description: Option<String>,
    value: String,
    #[serde(default, rename = "default")]
    is_default: bool,
}

#[derive(Deserialize)]
struct SettingsDto {
    #[serde(default)]
    show_resolved_model: Option<bool>,
    #[serde(default)]
    voice_mode_enabled: Option<bool>,
    #[serde(default)]
    sharing_enabled: Option<bool>,
    #[serde(default)]
    privacy_notice_rollout: Option<bool>,
    #[serde(default)]
    privacy_banner_reshow_days: Option<u64>,
    #[serde(default)]
    session_picker_grouped: Option<bool>,
    #[serde(default)]
    group_tool_verbs: Option<bool>,
    #[serde(default)]
    collapsed_edit_blocks: Option<bool>,
    #[serde(default)]
    subscription_watch_interval_secs: Option<u64>,
    #[serde(default)]
    gate_message: Option<String>,
    #[serde(default)]
    gate_url: Option<String>,
    #[serde(default)]
    gate_label: Option<String>,
    #[serde(default)]
    allow_access: Option<bool>,
    #[serde(default)]
    auto_permission_mode_enabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_presence_aware_string")]
    permission_mode: Option<Option<String>>,
}

fn normalize_settings(dto: SettingsDto, payload_keys: MetadataKeys) -> Option<GrokSettings> {
    let gate_message = bounded_optional_string(dto.gate_message, MAX_MESSAGE_BYTES);
    let gate_url = exact_optional_url(dto.gate_url)?;
    let gate_label = bounded_optional_string(dto.gate_label, MAX_DISPLAY_TEXT_BYTES);
    let access_gate = (gate_message.is_some() || gate_url.is_some() || gate_label.is_some())
        .then_some(AccessGate {
            message: gate_message,
            url: gate_url,
            label: gate_label,
        });

    Some(GrokSettings {
        permission_mode: match dto.permission_mode {
            None => OptionalSetting::NotReported,
            Some(None) => OptionalSetting::Cleared,
            Some(Some(value)) => OptionalSetting::Value(PermissionMode::from_wire(&value)),
        },
        auto_permission_mode_enabled: dto.auto_permission_mode_enabled,
        show_resolved_model: dto.show_resolved_model,
        voice_mode_enabled: dto.voice_mode_enabled,
        sharing_enabled: dto.sharing_enabled,
        privacy_notice_rollout: dto.privacy_notice_rollout,
        privacy_banner_reshow_days: dto.privacy_banner_reshow_days,
        session_picker_grouped: dto.session_picker_grouped,
        group_tool_verbs: dto.group_tool_verbs,
        collapsed_edit_blocks: dto.collapsed_edit_blocks,
        subscription_watch_interval_secs: dto.subscription_watch_interval_secs,
        access_allowed: dto.allow_access,
        access_gate,
        payload_keys,
    })
}

impl PermissionMode {
    fn from_wire(value: &str) -> Self {
        match value {
            "default" => Self::Default,
            "ask" => Self::Ask,
            "auto" => Self::Auto,
            "always-approve" => Self::AlwaysApprove,
            _ => Self::Other,
        }
    }
}

fn deserialize_presence_aware_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

#[derive(Deserialize)]
struct SessionChangesDto {
    #[serde(default)]
    upserted: BoundedVec<SessionSummaryDto, MAX_SESSION_CHANGES>,
    #[serde(default)]
    removed: BoundedVec<String, MAX_SESSION_CHANGES>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionSummaryDto {
    session_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    is_worktree: Option<bool>,
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    yolo: Option<bool>,
    #[serde(default)]
    activity: Option<String>,
    #[serde(default)]
    resident: Option<bool>,
    #[serde(default)]
    last_change_unix_ms: Option<i64>,
}

fn normalize_sessions(dto: SessionChangesDto) -> Option<SessionChanges> {
    let mut changed_ids = HashSet::new();
    let mut upserted = Vec::new();
    for session in dto.upserted.into_complete()? {
        let session_id = exact_bounded_string(session.session_id, MAX_IDENTIFIER_BYTES)?;
        if !changed_ids.insert(session_id.clone()) {
            return None;
        }
        upserted.push(SessionSummary {
            session_id,
            title: bounded_optional_string(session.title, MAX_DISPLAY_TEXT_BYTES),
            is_worktree: session.is_worktree,
            model_id: exact_optional_string(session.model_id, MAX_IDENTIFIER_BYTES)?,
            reasoning_effort: exact_optional_string(
                session.reasoning_effort,
                MAX_PROTOCOL_TOKEN_LEN,
            )?,
            always_approve: session.yolo,
            activity: SessionActivity::from_wire(session.activity.as_deref()),
            resident: session.resident,
            last_change_unix_ms: session.last_change_unix_ms,
        });
    }

    let mut removed_ids = HashSet::new();
    let mut removed_session_ids = Vec::new();
    for raw_session_id in dto.removed.into_complete()? {
        let session_id = exact_bounded_string(raw_session_id, MAX_IDENTIFIER_BYTES)?;
        if changed_ids.contains(&session_id) || !removed_ids.insert(session_id.clone()) {
            return None;
        }
        removed_session_ids.push(session_id);
    }

    Some(SessionChanges {
        upserted,
        removed_session_ids,
    })
}

impl SessionActivity {
    fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("working") => Self::Working,
            Some("idle") => Self::Idle,
            Some("needs_input") => Self::NeedsInput,
            Some("dormant") => Self::Dormant,
            Some("completed") => Self::Completed,
            Some("dead") => Self::Dead,
            _ => Self::Other,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueStateDto {
    session_id: String,
    #[serde(default)]
    entries: BoundedVec<QueueEntryDto, MAX_QUEUE_ENTRIES>,
    #[serde(default)]
    running_prompt_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueEntryDto {
    id: String,
    #[serde(default)]
    version: u64,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    position: usize,
}

fn normalize_queue(dto: QueueStateDto) -> Option<PromptQueueState> {
    let mut entry_ids = HashSet::new();
    let mut entries = Vec::new();
    for entry in dto.entries.into_complete()? {
        let id = exact_bounded_string(entry.id, MAX_IDENTIFIER_BYTES)?;
        if !entry_ids.insert(id.clone()) {
            return None;
        }
        entries.push(QueuedPrompt {
            id,
            version: entry.version,
            kind: match entry.kind.as_str() {
                "prompt" => QueueEntryKind::Prompt,
                "bash" => QueueEntryKind::Bash,
                _ => QueueEntryKind::Other,
            },
            position: entry.position,
        });
    }

    Some(PromptQueueState {
        session_id: exact_bounded_string(dto.session_id, MAX_IDENTIFIER_BYTES)?,
        entries,
        running_prompt_id: exact_optional_string(dto.running_prompt_id, MAX_IDENTIFIER_BYTES)?,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpInitProgressDto {
    total: u64,
    connected: u64,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpInitializedDto {
    session_id: String,
    #[serde(default)]
    mcp_tool_count: Option<u64>,
    #[serde(default)]
    elapsed_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServerStatusDto {
    session_id: String,
    name: String,
    #[serde(default)]
    source: Option<String>,
    status: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(rename = "detail", default, deserialize_with = "deserialize_present")]
    detail_present: bool,
    #[serde(default)]
    tools: Option<Value>,
}

fn deserialize_present<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

fn deserialize_optional_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    match Option::<Value>::deserialize(deserializer)? {
        None => Ok(None),
        Some(value @ Value::Object(_)) => serde_json::from_value(value)
            .map(Some)
            .map_err(|_| serde::de::Error::custom("malformed optional object")),
        Some(_) => Err(serde::de::Error::custom(
            "optional nested field must be an object or null",
        )),
    }
}

fn normalize_mcp_server_status(dto: McpServerStatusDto) -> Option<McpUpdate> {
    Some(McpUpdate::ServerStatus {
        session_id: exact_bounded_string(dto.session_id, MAX_IDENTIFIER_BYTES)?,
        name: exact_bounded_string(dto.name, MAX_IDENTIFIER_BYTES)?,
        source: McpServerSource::from_wire(dto.source.as_deref()),
        status: McpServerStatus::from_wire(Some(&dto.status)),
        reason: McpServerStatusReason::from_wire(dto.reason.as_deref()),
        detail_present: dto.detail_present,
        tool_count: optional_array_len(dto.tools.as_ref())?,
    })
}

fn optional_array_len(value: Option<&Value>) -> Option<Option<usize>> {
    match value {
        None | Some(Value::Null) => Some(None),
        Some(Value::Array(values)) => Some(Some(values.len())),
        Some(_) => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServersUpdatedDto {
    #[serde(default)]
    mcp_servers: BoundedVec<McpServerDto, MAX_MCP_SERVERS>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServerDto {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_object")]
    session: Option<McpServerSessionDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServerSessionDto {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    tools: Option<Value>,
    #[serde(default)]
    auth_required: Option<bool>,
    #[serde(default)]
    setup_required: Option<bool>,
}

fn normalize_mcp_servers(dto: McpServersUpdatedDto) -> Option<McpUpdate> {
    let mut server_names = HashSet::new();
    let mut servers = Vec::new();
    for server in dto.mcp_servers.into_complete()? {
        let name = exact_bounded_string(server.name, MAX_IDENTIFIER_BYTES)?;
        if !server_names.insert(name.clone()) {
            return None;
        }
        let session = server.session;
        servers.push(McpServerSummary {
            name,
            display_name: bounded_optional_string(server.display_name, MAX_DISPLAY_TEXT_BYTES),
            source: McpServerSource::from_wire(server.source.as_deref()),
            enabled: session.as_ref().and_then(|state| state.enabled),
            status: session
                .as_ref()
                .and_then(|state| state.status.as_deref())
                .map(|status| McpServerStatus::from_wire(Some(status))),
            tool_count: optional_array_len(
                session.as_ref().and_then(|state| state.tools.as_ref()),
            )?,
            auth_required: session.as_ref().and_then(|state| state.auth_required),
            setup_required: session.as_ref().and_then(|state| state.setup_required),
        });
    }

    Some(McpUpdate::ServersUpdated { servers })
}

impl McpServerSource {
    fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("managed") => Self::Managed,
            Some("local") => Self::Local,
            _ => Self::Other,
        }
    }
}

impl McpServerStatus {
    fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("ready") => Self::Ready,
            Some("initializing") => Self::Initializing,
            Some("setuprequired" | "setup_required") => Self::SetupRequired,
            Some("unavailable") => Self::Unavailable,
            Some("needsauth" | "needs_auth") => Self::NeedsAuth,
            _ => Self::Other,
        }
    }
}

impl McpServerStatusReason {
    fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("transport_closed") => Self::TransportClosed,
            Some("handshake_failed") => Self::HandshakeFailed,
            Some("config_added") => Self::ConfigAdded,
            Some("config_removed") => Self::ConfigRemoved,
            Some("config_changed") => Self::ConfigChanged,
            Some("disabled") => Self::Disabled,
            Some("auth_expired") => Self::AuthExpired,
            Some("initialized") => Self::Initialized,
            Some("restart_succeeded") => Self::RestartSucceeded,
            Some("restart_failed") => Self::RestartFailed,
            Some("managed_token_refreshed") => Self::ManagedTokenRefreshed,
            _ => Self::Other,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptCompletionDto {
    session_id: String,
    prompt_id: String,
    stop_reason: String,
    #[serde(default)]
    error_kind: Option<String>,
}

fn normalize_prompt_completion(dto: PromptCompletionDto) -> Option<PromptCompletion> {
    Some(PromptCompletion {
        session_id: exact_bounded_string(dto.session_id, MAX_IDENTIFIER_BYTES)?,
        prompt_id: exact_bounded_string(dto.prompt_id, MAX_IDENTIFIER_BYTES)?,
        stop_reason: PromptStopReason::from_wire(&dto.stop_reason),
        error_kind: dto
            .error_kind
            .filter(|value| is_safe_protocol_token(value))
            .map(|value| value.as_str().to_owned()),
    })
}

impl PromptStopReason {
    fn from_wire(value: &str) -> Self {
        match value {
            "end_turn" => Self::EndTurn,
            "tool_use" => Self::ToolUse,
            "stop_sequence" => Self::StopSequence,
            "max_tokens" => Self::MaxTokens,
            "max_turn_requests" => Self::MaxTurnRequests,
            "refusal" => Self::Refusal,
            "cancelled" => Self::Cancelled,
            "error" => Self::Error,
            _ => Self::Other,
        }
    }
}

fn is_safe_protocol_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PROTOCOL_TOKEN_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        && redact_diagnostic(value) == value
}

/// Read one optional aliased field without conflating a malformed value with
/// absence. Explicit `null` means "not reported" for these snapshot fields.
/// Supplying more than one spelling is ambiguous and therefore malformed.
fn optional_value_alias<'a>(
    object: &'a serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Option<Option<&'a Value>> {
    let mut found = None;
    for alias in aliases {
        if let Some(value) = object.get(*alias) {
            if found.is_some() {
                return None;
            }
            found = Some(value);
        }
    }

    match found {
        None | Some(Value::Null) => Some(None),
        Some(value) => Some(Some(value)),
    }
}

fn optional_value_field<'a>(
    object: Option<&'a serde_json::Map<String, Value>>,
    key: &str,
) -> Option<Option<&'a Value>> {
    let Some(object) = object else {
        return Some(None);
    };
    optional_value_alias(object, &[key])
}

fn optional_str_field<'a>(
    object: Option<&'a serde_json::Map<String, Value>>,
    key: &str,
) -> Option<Option<&'a str>> {
    match optional_value_field(object, key)? {
        None => Some(None),
        Some(value) => value.as_str().map(Some),
    }
}

fn optional_array_field<'a>(
    object: Option<&'a serde_json::Map<String, Value>>,
    key: &str,
) -> Option<Option<&'a Vec<Value>>> {
    match optional_value_field(object, key)? {
        None => Some(None),
        Some(value) => value.as_array().map(Some),
    }
}

fn optional_bool_field(
    object: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<Option<bool>> {
    match optional_value_field(object, key)? {
        None => Some(None),
        Some(value) => value.as_bool().map(Some),
    }
}

fn optional_u64_field(
    object: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<Option<u64>> {
    match optional_value_field(object, key)? {
        None => Some(None),
        Some(value) => value.as_u64().map(Some),
    }
}

fn u64_alias(object: &serde_json::Map<String, Value>, aliases: &[&str]) -> Option<Option<u64>> {
    match optional_value_alias(object, aliases)? {
        None => Some(None),
        Some(value) => value.as_u64().map(Some),
    }
}

fn i64_alias(object: &serde_json::Map<String, Value>, aliases: &[&str]) -> Option<Option<i64>> {
    match optional_value_alias(object, aliases)? {
        None => Some(None),
        Some(value) => value.as_i64().map(Some),
    }
}

fn bool_alias(object: &serde_json::Map<String, Value>, aliases: &[&str]) -> Option<Option<bool>> {
    match optional_value_alias(object, aliases)? {
        None => Some(None),
        Some(value) => value.as_bool().map(Some),
    }
}

fn percentage_alias(
    object: &serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Option<Option<u8>> {
    match u64_alias(object, aliases)? {
        None => Some(None),
        Some(value @ 0..=100) => Some(Some(value as u8)),
        Some(_) => None,
    }
}

fn str_alias<'a>(
    object: &'a serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Option<Option<&'a str>> {
    match optional_value_alias(object, aliases)? {
        None => Some(None),
        Some(value) => value.as_str().map(Some),
    }
}

fn object_alias<'a>(
    object: &'a serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Option<Option<&'a serde_json::Map<String, Value>>> {
    match optional_value_alias(object, aliases)? {
        None => Some(None),
        Some(value) => value.as_object().map(Some),
    }
}

fn bounded_string_alias(
    object: &serde_json::Map<String, Value>,
    aliases: &[&str],
    maximum_bytes: usize,
) -> Option<Option<String>> {
    Some(str_alias(object, aliases)?.map(|value| bounded_str(value, maximum_bytes)))
}

fn normalize_usage(value: Option<&Value>) -> Option<Option<SessionUsage>> {
    let Some(value) = value else {
        return Some(None);
    };
    if value.is_null() {
        return Some(None);
    }
    let object = value.as_object()?;
    let usage = SessionUsage {
        input_tokens: u64_alias(object, &["inputTokens", "input_tokens"])?,
        output_tokens: u64_alias(object, &["outputTokens", "output_tokens"])?,
        total_tokens: u64_alias(object, &["totalTokens", "total_tokens"])?,
        cached_read_tokens: u64_alias(
            object,
            &[
                "cachedReadTokens",
                "cached_read_tokens",
                "cacheReadInputTokens",
                "cache_read_input_tokens",
            ],
        )?,
        cache_creation_tokens: u64_alias(
            object,
            &[
                "cacheCreationTokens",
                "cache_creation_tokens",
                "cacheCreationInputTokens",
                "cache_creation_input_tokens",
            ],
        )?,
        reasoning_tokens: u64_alias(object, &["reasoningTokens", "reasoning_tokens"])?,
        model_calls: u64_alias(object, &["modelCalls", "model_calls"])?,
        api_duration_ms: u64_alias(object, &["apiDurationMs", "api_duration_ms"])?,
        turn_count: u64_alias(object, &["numTurns", "num_turns"])?,
        incomplete: bool_alias(object, &["usageIsIncomplete", "usage_is_incomplete"])?,
    };

    Some((usage != SessionUsage::default()).then_some(usage))
}

fn merge_optional_equal<T: Copy + Eq>(
    nested: Option<T>,
    flattened: Option<T>,
) -> Option<Option<T>> {
    match (nested, flattened) {
        (Some(left), Some(right)) if left != right => None,
        (Some(value), _) | (_, Some(value)) => Some(Some(value)),
        (None, None) => Some(None),
    }
}

fn normalize_session_status(update: &serde_json::Map<String, Value>) -> Option<SessionStatus> {
    let model = object_alias(update, &["model"])?;
    let effort = object_alias(update, &["effort"])?;
    let context_window = object_alias(update, &["contextWindow", "context_window"])?;
    let turn = object_alias(update, &["turn"])?;

    let usage = if let Some(context) = context_window {
        let mut usage = normalize_usage(optional_value_alias(
            context,
            &["sessionUsage", "session_usage"],
        )?)?
        .unwrap_or_default();
        usage.input_tokens = merge_optional_equal(
            usage.input_tokens,
            u64_alias(context, &["sessionInputTokens", "session_input_tokens"])?,
        )?;
        usage.output_tokens = merge_optional_equal(
            usage.output_tokens,
            u64_alias(context, &["sessionOutputTokens", "session_output_tokens"])?,
        )?;
        (usage != SessionUsage::default()).then_some(usage)
    } else {
        None
    };

    Some(SessionStatus {
        schema_version: u64_alias(update, &["schemaVersion", "schema_version"])?,
        model_id: exact_optional_str(
            match model {
                Some(value) => str_alias(value, &["id"])?,
                None => None,
            },
            MAX_IDENTIFIER_BYTES,
        )?,
        model_name: match model {
            Some(value) => bounded_string_alias(
                value,
                &["displayName", "display_name"],
                MAX_DISPLAY_TEXT_BYTES,
            )?,
            None => None,
        },
        reasoning_effort: exact_optional_str(
            match effort {
                Some(value) => str_alias(value, &["level"])?,
                None => None,
            },
            MAX_PROTOCOL_TOKEN_LEN,
        )?,
        context_window_tokens: match context_window {
            Some(context) => u64_alias(context, &["contextWindowSize", "context_window_size"])?,
            None => None,
        },
        context_tokens: match context_window {
            Some(context) => u64_alias(context, &["contextTokens", "context_tokens"])?,
            None => None,
        },
        usage,
        used_percentage: match context_window {
            Some(context) => percentage_alias(context, &["usedPercentage", "used_percentage"])?,
            None => None,
        },
        remaining_percentage: match context_window {
            Some(context) => {
                percentage_alias(context, &["remainingPercentage", "remaining_percentage"])?
            }
            None => None,
        },
        auto_compact_threshold_percentage: match context_window {
            Some(context) => percentage_alias(
                context,
                &[
                    "autoCompactThresholdPercent",
                    "auto_compact_threshold_percent",
                ],
            )?,
            None => None,
        },
        turn_started_at_ms: match turn {
            Some(value) => i64_alias(value, &["startedAtMs", "started_at_ms"])?,
            None => None,
        },
        trigger: str_alias(update, &["trigger"])?.map(SessionStatusTrigger::from_wire),
        payload_keys: MetadataKeys::from_object(update),
    })
}

impl SessionStatusTrigger {
    fn from_wire(value: &str) -> Self {
        match value {
            "state" => Self::State,
            "refresh_interval" => Self::RefreshInterval,
            _ => Self::Other,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionEnvelopeDto {
    session_id: String,
    update: Value,
    #[serde(rename = "_meta", default)]
    metadata: Option<serde_json::Map<String, Value>>,
}

fn normalize_session_update(
    payload: Value,
    rail: SessionUpdateRail,
    malformed: KnownExtensionMalformed,
) -> GrokExtensionOutcome {
    let Some(dto) = decode::<SessionEnvelopeDto>(payload) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(update) = dto.update.as_object() else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(kind) = update.get("sessionUpdate").and_then(Value::as_str) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };

    let metadata_keys = dto
        .metadata
        .as_ref()
        .map_or_else(MetadataKeys::default, MetadataKeys::from_object);
    let replay = match dto.metadata.as_ref() {
        Some(metadata) => {
            let Some(value) = optional_bool_field(Some(metadata), "isReplay") else {
                return GrokExtensionOutcome::Malformed(malformed);
            };
            value.unwrap_or(false)
        }
        None => false,
    };
    let Some(stop_reason_value) = str_alias(update, &["stopReason", "stop_reason"]) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let stop_reason = stop_reason_value.map(PromptStopReason::from_wire);
    let kind = SessionUpdateKind::from_wire(kind);
    let Some(error_kind_value) = str_alias(update, &["errorKind", "error_kind"]) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let error_kind = error_kind_value
        .filter(|value| is_safe_protocol_token(value))
        .map(str::to_owned);
    let Some(usage_value) = optional_value_alias(update, &["usage"]) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(usage) = normalize_usage(usage_value) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let status = if kind == SessionUpdateKind::SessionStatus {
        let Some(status) = normalize_session_status(update) else {
            return GrokExtensionOutcome::Malformed(malformed);
        };
        Some(status)
    } else {
        None
    };
    let Some(session_id) = exact_bounded_string(dto.session_id, MAX_IDENTIFIER_BYTES) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(prompt_id_value) = str_alias(update, &["promptId", "prompt_id"]) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(prompt_id) = exact_optional_str(prompt_id_value, MAX_IDENTIFIER_BYTES) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(model_id_value) = str_alias(update, &["modelId", "model_id"]) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(model_id) = exact_optional_str(model_id_value, MAX_IDENTIFIER_BYTES) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(reasoning_effort_value) = str_alias(update, &["reasoningEffort", "reasoning_effort"])
    else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(reasoning_effort) = exact_optional_str(reasoning_effort_value, MAX_PROTOCOL_TOKEN_LEN)
    else {
        return GrokExtensionOutcome::Malformed(malformed);
    };
    let Some(elapsed_ms) = u64_alias(update, &["elapsedMs", "elapsed_ms"]) else {
        return GrokExtensionOutcome::Malformed(malformed);
    };

    GrokExtensionOutcome::SessionUpdated(Box::new(SessionExtensionUpdate {
        session_id,
        rail,
        kind,
        prompt_id,
        stop_reason,
        error_kind,
        elapsed_ms,
        model_id,
        reasoning_effort,
        usage,
        status,
        replay,
        metadata_keys,
        update_keys: MetadataKeys::from_value(&dto.update),
    }))
}

impl SessionUpdateKind {
    fn from_wire(value: &str) -> Self {
        match value {
            "turn_completed" => Self::TurnCompleted,
            "response_started" => Self::ResponseStarted,
            "response_completed" => Self::ResponseCompleted,
            "reasoning_completed" => Self::ReasoningCompleted,
            "model_changed" => Self::ModelChanged,
            "session_status" => Self::SessionStatus,
            "pending_interaction" => Self::PendingInteraction,
            "interaction_resolved" => Self::InteractionResolved,
            "session_summary_generated" => Self::SessionSummaryGenerated,
            "tool_call_delta_chunk" => Self::ToolCallDeltaChunk,
            "hook_execution" => Self::HookExecution,
            "subagent_spawned" => Self::SubagentSpawned,
            "subagent_finished" => Self::SubagentFinished,
            _ => Self::Other,
        }
    }
}

#[derive(Deserialize)]
struct AnnouncementBatchDto {
    #[serde(rename = "gen")]
    generation: u64,
    #[serde(default)]
    announcements: BoundedVec<AnnouncementDto, MAX_ANNOUNCEMENTS>,
}

#[derive(Deserialize)]
struct AnnouncementDto {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    dismissible: Option<bool>,
    #[serde(default)]
    persistent: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_object")]
    cta: Option<AnnouncementCallToActionDto>,
}

#[derive(Deserialize)]
struct AnnouncementCallToActionDto {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    caption: Option<String>,
}

fn normalize_announcements(dto: AnnouncementBatchDto) -> Option<AnnouncementBatch> {
    Some(AnnouncementBatch {
        generation: dto.generation,
        announcements: dto
            .announcements
            .into_complete()?
            .into_iter()
            .map(|announcement| {
                let call_to_action = match announcement.cta {
                    Some(cta) => Some(AnnouncementCallToAction {
                        label: bounded_optional_string(cta.label, MAX_DISPLAY_TEXT_BYTES),
                        url: exact_optional_url(cta.url)?,
                        caption: bounded_optional_string(cta.caption, MAX_DESCRIPTION_BYTES),
                    }),
                    None => None,
                };
                Some(Announcement {
                    id: exact_optional_string(announcement.id, MAX_IDENTIFIER_BYTES)?,
                    title: bounded_optional_string(announcement.title, MAX_DISPLAY_TEXT_BYTES),
                    message: bounded_optional_string(announcement.message, MAX_MESSAGE_BYTES),
                    severity: announcement
                        .severity
                        .and_then(|value| exact_bounded_string(value, MAX_PROTOCOL_TOKEN_LEN)),
                    dismissible: announcement.dismissible,
                    persistent: announcement.persistent,
                    call_to_action,
                })
            })
            .collect::<Option<Vec<_>>>()?,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn oversized(prefix: &str, maximum_bytes: usize) -> String {
        format!("{prefix}{}", "x".repeat(maximum_bytes + 32))
    }

    fn assert_at_most(value: &str, maximum_bytes: usize) {
        assert!(
            value.len() <= maximum_bytes,
            "{} bytes exceeds {maximum_bytes}",
            value.len()
        );
    }

    #[test]
    fn safe_external_urls_remain_actionable_without_credential_channels() {
        let safe = "https://example.invalid/access";
        assert!(is_safe_external_url(safe));
        assert_eq!(exact_bounded_url(safe.to_owned()).as_deref(), Some(safe));
        assert!(is_safe_external_url("http://localhost:8080/setup"));

        for unsafe_url in [
            "file:///home/private/secret",
            "https://user:secret@example.invalid/access",
            "https://example.invalid/access?token=private",
            "https://example.invalid/%2fhome%2fprivate",
            "https://example.invalid/C:/Users/Private",
            "https://example.invalid/home/private/project",
            "https://example.invalid/api_key/private",
            "https://example.invalid:/access",
            "https://example.invalid:99999/access",
        ] {
            assert!(exact_bounded_url(unsafe_url.to_owned()).is_none());
        }
    }

    #[test]
    fn model_catalog_and_nested_reasoning_options_are_bounded_before_projection() {
        let long_display = oversized("display-", MAX_DISPLAY_TEXT_BYTES);
        let long_description = oversized("description-", MAX_DESCRIPTION_BYTES);
        let reasoning_efforts = (0..MAX_REASONING_EFFORTS)
            .map(|index| {
                json!({
                    "id": format!("id-{index}"),
                    "label": if index == 0 { long_display.clone() } else { format!("label-{index}") },
                    "description": if index == 0 { long_description.clone() } else { format!("description-{index}") },
                    "value": format!("value-{index}"),
                    "default": index == 0
                })
            })
            .collect::<Vec<_>>();
        let mut models = (0..MAX_MODELS)
            .map(|index| {
                json!({
                    "modelId": format!("model-{index}"),
                    "name": format!("Model {index}")
                })
            })
            .collect::<Vec<_>>();
        models[0] = json!({
            "modelId": "model-0",
            "name": long_display.clone(),
            "description": long_description.clone(),
            "_meta": {
                "agentType": "build",
                "reasoningEffort": "value-0",
                "reasoningEfforts": reasoning_efforts
            }
        });

        let outcome = normalize_grok_extension(
            "_x.ai/models/update",
            json!({
                "currentModelId": "model-0",
                "availableModels": models
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::ModelsUpdated(catalog) = outcome else {
            panic!("expected bounded model catalog");
        };

        assert_eq!(catalog.available_models.len(), MAX_MODELS);
        assert_at_most(&catalog.current_model_id, MAX_IDENTIFIER_BYTES);
        let model = &catalog.available_models[0];
        assert_at_most(&model.model_id, MAX_IDENTIFIER_BYTES);
        assert_at_most(&model.name, MAX_DISPLAY_TEXT_BYTES);
        assert_at_most(
            model.description.as_deref().expect("description"),
            MAX_DESCRIPTION_BYTES,
        );
        assert_at_most(
            model.agent_type.as_deref().expect("agent type"),
            MAX_PROTOCOL_TOKEN_LEN,
        );
        assert_at_most(
            model.reasoning_effort.as_deref().expect("reasoning effort"),
            MAX_PROTOCOL_TOKEN_LEN,
        );
        assert_eq!(model.reasoning_efforts.len(), MAX_REASONING_EFFORTS);
        let option = &model.reasoning_efforts[0];
        assert_at_most(&option.id, MAX_PROTOCOL_TOKEN_LEN);
        assert_at_most(&option.label, MAX_DISPLAY_TEXT_BYTES);
        assert_at_most(
            option.description.as_deref().expect("option description"),
            MAX_DESCRIPTION_BYTES,
        );
        assert_at_most(&option.value, MAX_PROTOCOL_TOKEN_LEN);

        let overflow_models = (0..MAX_MODELS + 1)
            .map(|index| json!({"modelId": format!("m-{index}"), "name": "Model"}))
            .collect::<Vec<_>>();
        let overflow = normalize_grok_extension(
            "_x.ai/models/update",
            json!({"currentModelId": "m-0", "availableModels": overflow_models}),
        )
        .unwrap();
        assert!(matches!(overflow, GrokExtensionOutcome::Malformed(_)));

        let overflow_options = (0..MAX_REASONING_EFFORTS + 1)
            .map(|index| {
                json!({
                    "id": format!("e-{index}"),
                    "label": "Effort",
                    "value": format!("value-{index}")
                })
            })
            .collect::<Vec<_>>();
        let overflow = normalize_grok_extension(
            "_x.ai/models/update",
            json!({
                "currentModelId": "m",
                "availableModels": [{
                    "modelId": "m",
                    "name": "Model",
                    "_meta": {"reasoningEfforts": overflow_options}
                }]
            }),
        )
        .unwrap();
        assert!(matches!(overflow, GrokExtensionOutcome::Malformed(_)));
    }

    #[test]
    fn model_catalog_rejects_unresolved_and_duplicate_snapshot_identity() {
        let cases = [
            json!({"currentModelId": "missing", "availableModels": []}),
            json!({
                "currentModelId": "missing",
                "availableModels": [{"modelId": "other", "name": "Other"}]
            }),
            json!({
                "currentModelId": "same",
                "availableModels": [
                    {"modelId": "same", "name": "First"},
                    {"modelId": "same", "name": "Second"}
                ]
            }),
            json!({
                "currentModelId": "model",
                "availableModels": [{"modelId": "", "name": "Empty"}]
            }),
            json!({
                "currentModelId": "model",
                "availableModels": [{
                    "modelId": "model",
                    "name": "Model",
                    "_meta": {"reasoningEfforts": [
                        {"id": "same", "label": "First", "value": "low"},
                        {"id": "same", "label": "Second", "value": "high"}
                    ]}
                }]
            }),
            json!({
                "currentModelId": "model",
                "availableModels": [{
                    "modelId": "model",
                    "name": "Model",
                    "_meta": {"reasoningEfforts": [
                        {"id": "low", "label": "First", "value": "same"},
                        {"id": "high", "label": "Second", "value": "same"}
                    ]}
                }]
            }),
        ];

        for payload in cases {
            let outcome = normalize_grok_extension("_x.ai/models/update", payload).unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        }
    }

    #[test]
    fn selected_reasoning_effort_must_resolve_and_default_must_be_unique() {
        let malformed_metadata = [
            json!({
                "reasoningEffort": "bogus",
                "reasoningEfforts": [
                    {"id": "high", "label": "High", "value": "high"}
                ]
            }),
            json!({"reasoningEffort": "high"}),
            json!({"reasoningEffort": "high", "reasoningEfforts": null}),
            json!({
                "reasoningEffort": "high",
                "reasoningEfforts": [
                    {"id": "low", "label": "Low", "value": "low", "default": true},
                    {"id": "high", "label": "High", "value": "high", "default": true}
                ]
            }),
        ];
        for metadata in malformed_metadata {
            let outcome = normalize_grok_extension(
                "_x.ai/models/update",
                json!({
                    "currentModelId": "model",
                    "availableModels": [{
                        "modelId": "model",
                        "name": "Model",
                        "_meta": metadata
                    }]
                }),
            )
            .unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        }

        let valid = normalize_grok_extension(
            "_x.ai/models/update",
            json!({
                "currentModelId": "model",
                "availableModels": [{
                    "modelId": "model",
                    "name": "Model",
                    "_meta": {
                        "reasoningEffort": "high",
                        "reasoningEfforts": [
                            {"id": "low", "label": "Low", "value": "low"},
                            {"id": "high", "label": "High", "value": "high", "default": true}
                        ]
                    }
                }]
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::ModelsUpdated(catalog) = valid else {
            panic!("expected consistent reasoning snapshot");
        };
        assert_eq!(
            catalog.available_models[0].reasoning_effort.as_deref(),
            Some("high")
        );

        let nulls = normalize_grok_extension(
            "_x.ai/models/update",
            json!({
                "currentModelId": "model",
                "availableModels": [{
                    "modelId": "model",
                    "name": "Model",
                    "_meta": {"reasoningEffort": null, "reasoningEfforts": null}
                }]
            }),
        )
        .unwrap();
        assert!(matches!(nulls, GrokExtensionOutcome::ModelsUpdated(_)));
    }

    #[test]
    fn model_metadata_distinguishes_null_from_wrong_typed_known_fields() {
        let nulls = normalize_grok_extension(
            "_x.ai/models/update",
            json!({
                "currentModelId": "model",
                "availableModels": [{
                    "modelId": "model",
                    "name": "Model",
                    "_meta": {
                        "agentType": null,
                        "reasoningEffort": null,
                        "reasoningEfforts": null,
                        "supportsReasoningEffort": null,
                        "totalContextTokens": null
                    }
                }]
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::ModelsUpdated(catalog) = nulls else {
            panic!("optional model metadata nulls must remain absent");
        };
        let model = &catalog.available_models[0];
        assert_eq!(model.agent_type, None);
        assert_eq!(model.reasoning_effort, None);
        assert!(model.reasoning_efforts.is_empty());
        assert_eq!(model.supports_reasoning_effort, None);
        assert_eq!(model.total_context_tokens, None);

        let malformed_metadata = [
            json!("not-an-object"),
            json!({"agentType": false}),
            json!({"reasoningEffort": 1}),
            json!({"reasoningEfforts": {}}),
            json!({"supportsReasoningEffort": "yes"}),
            json!({"totalContextTokens": -1}),
        ];
        for metadata in malformed_metadata {
            let outcome = normalize_grok_extension(
                "_x.ai/models/update",
                json!({
                    "currentModelId": "model",
                    "availableModels": [{
                        "modelId": "model",
                        "name": "Model",
                        "_meta": metadata
                    }]
                }),
            )
            .unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        }
    }

    #[test]
    fn session_queue_and_mcp_collections_and_strings_are_bounded() {
        let long_display = oversized("display-", MAX_DISPLAY_TEXT_BYTES);

        let mut upserted = (0..MAX_SESSION_CHANGES)
            .map(|index| json!({"sessionId": format!("s-{index}")}))
            .collect::<Vec<_>>();
        upserted[0] = json!({
            "sessionId": "s-0",
            "title": long_display,
            "modelId": "model-0",
            "reasoningEffort": "high"
        });
        let removed = (0..MAX_SESSION_CHANGES)
            .map(|index| format!("removed-{index}"))
            .collect::<Vec<_>>();
        let sessions = normalize_grok_extension(
            "_x.ai/sessions/changed",
            json!({"upserted": upserted, "removed": removed}),
        )
        .unwrap();
        let GrokExtensionOutcome::SessionsChanged(sessions) = sessions else {
            panic!("expected bounded session changes");
        };
        assert_eq!(sessions.upserted.len(), MAX_SESSION_CHANGES);
        assert_eq!(sessions.removed_session_ids.len(), MAX_SESSION_CHANGES);
        assert_at_most(&sessions.upserted[0].session_id, MAX_IDENTIFIER_BYTES);
        assert_at_most(
            sessions.upserted[0].title.as_deref().expect("title"),
            MAX_DISPLAY_TEXT_BYTES,
        );
        assert_at_most(
            sessions.upserted[0].model_id.as_deref().expect("model"),
            MAX_IDENTIFIER_BYTES,
        );
        assert_at_most(
            sessions.upserted[0]
                .reasoning_effort
                .as_deref()
                .expect("reasoning effort"),
            MAX_PROTOCOL_TOKEN_LEN,
        );
        assert!(
            sessions
                .removed_session_ids
                .iter()
                .all(|id| id.len() <= MAX_IDENTIFIER_BYTES)
        );

        let mut entries = (0..MAX_QUEUE_ENTRIES)
            .map(|index| json!({"id": format!("q-{index}"), "kind": "prompt"}))
            .collect::<Vec<_>>();
        entries[0]["id"] = json!("q-0");
        let queue = normalize_grok_extension(
            "_x.ai/queue/changed",
            json!({
                "sessionId": "session-0",
                "entries": entries,
                "runningPromptId": "q-0"
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::QueueChanged(queue) = queue else {
            panic!("expected bounded queue");
        };
        assert_eq!(queue.entries.len(), MAX_QUEUE_ENTRIES);
        assert_at_most(&queue.session_id, MAX_IDENTIFIER_BYTES);
        assert_at_most(&queue.entries[0].id, MAX_IDENTIFIER_BYTES);
        assert_at_most(
            queue.running_prompt_id.as_deref().expect("running prompt"),
            MAX_IDENTIFIER_BYTES,
        );

        let mut servers = (0..MAX_MCP_SERVERS)
            .map(|index| json!({"name": format!("server-{index}")}))
            .collect::<Vec<_>>();
        servers[0] = json!({
            "name": "server-0",
            "displayName": oversized("Server ", MAX_DISPLAY_TEXT_BYTES),
            "session": {
                "status": "ready",
                "tools": [{}, {}, {}]
            }
        });
        let mcp =
            normalize_grok_extension("_x.ai/mcp/servers_updated", json!({"mcpServers": servers}))
                .unwrap();
        let GrokExtensionOutcome::McpChanged(McpUpdate::ServersUpdated { servers }) = mcp else {
            panic!("expected bounded MCP servers");
        };
        assert_eq!(servers.len(), MAX_MCP_SERVERS);
        assert_at_most(&servers[0].name, MAX_IDENTIFIER_BYTES);
        assert_at_most(
            servers[0].display_name.as_deref().expect("display name"),
            MAX_DISPLAY_TEXT_BYTES,
        );
        assert_eq!(servers[0].tool_count, Some(3));
    }

    #[test]
    fn session_queue_and_mcp_snapshots_reject_ambiguous_duplicate_identities() {
        for payload in [
            json!({
                "upserted": [{"sessionId": "same"}, {"sessionId": "same"}],
                "removed": []
            }),
            json!({"upserted": [], "removed": ["same", "same"]}),
            json!({"upserted": [{"sessionId": "same"}], "removed": ["same"]}),
        ] {
            let outcome = normalize_grok_extension("_x.ai/sessions/changed", payload).unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        }

        let queue = normalize_grok_extension(
            "_x.ai/queue/changed",
            json!({
                "sessionId": "session-0",
                "entries": [
                    {"id": "same", "kind": "prompt"},
                    {"id": "same", "kind": "bash"}
                ]
            }),
        )
        .unwrap();
        assert!(matches!(queue, GrokExtensionOutcome::Malformed(_)));

        let mcp = normalize_grok_extension(
            "_x.ai/mcp/servers_updated",
            json!({"mcpServers": [{"name": "same"}, {"name": "same"}]}),
        )
        .unwrap();
        assert!(matches!(mcp, GrokExtensionOutcome::Malformed(_)));
    }

    #[test]
    fn replacement_and_delta_collection_overflow_is_explicitly_malformed() {
        let sessions = (0..MAX_SESSION_CHANGES + 1)
            .map(|index| json!({"sessionId": format!("s-{index}")}))
            .collect::<Vec<_>>();
        let queue = (0..MAX_QUEUE_ENTRIES + 1)
            .map(|index| json!({"id": format!("q-{index}")}))
            .collect::<Vec<_>>();
        let servers = (0..MAX_MCP_SERVERS + 1)
            .map(|index| json!({"name": format!("server-{index}")}))
            .collect::<Vec<_>>();
        let announcements = (0..MAX_ANNOUNCEMENTS + 1)
            .map(|index| json!({"id": format!("announcement-{index}")}))
            .collect::<Vec<_>>();

        for (method, payload) in [
            (
                "_x.ai/sessions/changed",
                json!({"upserted": sessions, "removed": []}),
            ),
            (
                "_x.ai/queue/changed",
                json!({"sessionId": "s", "entries": queue}),
            ),
            ("_x.ai/mcp/servers_updated", json!({"mcpServers": servers})),
            (
                "_x.ai/announcements/update",
                json!({"gen": 1, "announcements": announcements}),
            ),
        ] {
            let outcome = normalize_grok_extension(method, payload).unwrap();
            assert!(
                matches!(outcome, GrokExtensionOutcome::Malformed(_)),
                "{method} must not present a partial collection as complete"
            );
        }
    }

    #[test]
    fn remaining_extension_strings_and_announcements_are_bounded() {
        let settings = normalize_grok_extension(
            "_x.ai/settings/update",
            json!({
                "gate_message": oversized("message-", MAX_MESSAGE_BYTES),
                "gate_url": "https://example.invalid/access",
                "gate_label": oversized("label-", MAX_DISPLAY_TEXT_BYTES)
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SettingsUpdated(settings) = settings else {
            panic!("expected bounded settings");
        };
        let gate = settings.access_gate.expect("access gate");
        assert_at_most(gate.message.as_deref().expect("message"), MAX_MESSAGE_BYTES);
        assert_eq!(gate.url.as_deref(), Some("https://example.invalid/access"));
        assert_at_most(
            gate.label.as_deref().expect("label"),
            MAX_DISPLAY_TEXT_BYTES,
        );

        let prompt = normalize_grok_extension(
            "_x.ai/session/prompt_complete",
            json!({
                "sessionId": "session-0",
                "promptId": "prompt-0",
                "stopReason": "error",
                "errorKind": oversized("error-", MAX_PROTOCOL_TOKEN_LEN)
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::PromptCompleted(prompt) = prompt else {
            panic!("expected bounded prompt completion");
        };
        assert_at_most(&prompt.session_id, MAX_IDENTIFIER_BYTES);
        assert_at_most(&prompt.prompt_id, MAX_IDENTIFIER_BYTES);
        assert_eq!(prompt.error_kind, None);

        let update = normalize_grok_extension(
            "_x.ai/session_notification",
            json!({
                "sessionId": "session-0",
                "update": {
                    "sessionUpdate": "session_status",
                    "promptId": "prompt-0",
                    "modelId": "model-0",
                    "reasoningEffort": "high",
                    "model": {
                        "id": "model-0",
                        "displayName": oversized("Status model ", MAX_DISPLAY_TEXT_BYTES)
                    },
                    "effort": {"level": "high"}
                }
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SessionUpdated(update) = update else {
            panic!("expected bounded session update");
        };
        assert_at_most(&update.session_id, MAX_IDENTIFIER_BYTES);
        assert_at_most(
            update.prompt_id.as_deref().expect("prompt ID"),
            MAX_IDENTIFIER_BYTES,
        );
        assert_at_most(
            update.model_id.as_deref().expect("model ID"),
            MAX_IDENTIFIER_BYTES,
        );
        assert_at_most(
            update
                .reasoning_effort
                .as_deref()
                .expect("reasoning effort"),
            MAX_PROTOCOL_TOKEN_LEN,
        );
        let status = update.status.expect("session status");
        assert_at_most(
            status.model_id.as_deref().expect("status model ID"),
            MAX_IDENTIFIER_BYTES,
        );
        assert_at_most(
            status.model_name.as_deref().expect("status model name"),
            MAX_DISPLAY_TEXT_BYTES,
        );
        assert_at_most(
            status.reasoning_effort.as_deref().expect("status effort"),
            MAX_PROTOCOL_TOKEN_LEN,
        );

        let mut announcements = (0..MAX_ANNOUNCEMENTS)
            .map(|index| json!({"id": format!("announcement-{index}")}))
            .collect::<Vec<_>>();
        announcements[0] = json!({
            "id": "announcement-0",
            "title": oversized("title-", MAX_DISPLAY_TEXT_BYTES),
            "message": oversized("message-", MAX_MESSAGE_BYTES),
            "severity": "info",
            "cta": {
                "label": oversized("label-", MAX_DISPLAY_TEXT_BYTES),
                "url": "https://example.invalid/action",
                "caption": oversized("caption-", MAX_DESCRIPTION_BYTES)
            }
        });
        let announcements = normalize_grok_extension(
            "_x.ai/announcements/update",
            json!({"gen": 1, "announcements": announcements}),
        )
        .unwrap();
        let GrokExtensionOutcome::AnnouncementsUpdated(batch) = announcements else {
            panic!("expected bounded announcements");
        };
        assert_eq!(batch.announcements.len(), MAX_ANNOUNCEMENTS);
        let announcement = &batch.announcements[0];
        assert_at_most(
            announcement.id.as_deref().expect("announcement ID"),
            MAX_IDENTIFIER_BYTES,
        );
        assert_at_most(
            announcement.title.as_deref().expect("title"),
            MAX_DISPLAY_TEXT_BYTES,
        );
        assert_at_most(
            announcement.message.as_deref().expect("message"),
            MAX_MESSAGE_BYTES,
        );
        assert_at_most(
            announcement.severity.as_deref().expect("severity"),
            MAX_PROTOCOL_TOKEN_LEN,
        );
        let cta = announcement.call_to_action.as_ref().expect("CTA");
        assert_at_most(
            cta.label.as_deref().expect("CTA label"),
            MAX_DISPLAY_TEXT_BYTES,
        );
        assert_at_most(cta.url.as_deref().expect("CTA URL"), MAX_URL_BYTES);
        assert_at_most(
            cta.caption.as_deref().expect("CTA caption"),
            MAX_DESCRIPTION_BYTES,
        );

        let multibyte = format!("{}💖", "a".repeat(MAX_IDENTIFIER_BYTES - 1));
        let bounded = bounded_string(multibyte, MAX_IDENTIFIER_BYTES);
        assert_eq!(bounded.len(), MAX_IDENTIFIER_BYTES - 1);
        assert!(bounded.is_char_boundary(bounded.len()));

        let mut overallocated = String::with_capacity(MAX_MESSAGE_BYTES * 64);
        overallocated.push_str("short display text");
        let original_capacity = overallocated.capacity();
        let bounded = bounded_string(overallocated, MAX_DISPLAY_TEXT_BYTES);
        assert_eq!(bounded, "short display text");
        assert!(bounded.capacity() <= MAX_DISPLAY_TEXT_BYTES);
        assert!(bounded.capacity() < original_capacity);

        let mut overallocated_id = String::with_capacity(MAX_IDENTIFIER_BYTES * 1_024);
        overallocated_id.push_str("session-0");
        let original_capacity = overallocated_id.capacity();
        let bounded_id = exact_bounded_string(overallocated_id, MAX_IDENTIFIER_BYTES).unwrap();
        assert_eq!(bounded_id, "session-0");
        assert!(bounded_id.capacity() <= MAX_IDENTIFIER_BYTES);
        assert!(bounded_id.capacity() < original_capacity);
    }

    #[test]
    fn mcp_lifecycle_identifiers_are_exact_and_tool_counts_preserve_cardinality() {
        let progress = normalize_grok_extension(
            "_x.ai/mcp/init_progress",
            json!({
                "sessionId": "session-0",
                "total": 1,
                "connected": 0
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::McpChanged(McpUpdate::InitProgress {
            session_id: Some(session_id),
            ..
        }) = progress
        else {
            panic!("expected MCP init progress");
        };
        assert_at_most(&session_id, MAX_IDENTIFIER_BYTES);

        let initialized = normalize_grok_extension(
            "_x.ai/mcp_initialized",
            json!({
                "sessionId": "session-0",
                "mcpToolCount": 7
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::McpChanged(McpUpdate::Initialized { session_id, .. }) =
            initialized
        else {
            panic!("expected MCP initialized");
        };
        assert_at_most(&session_id, MAX_IDENTIFIER_BYTES);

        let status = normalize_grok_extension(
            "_x.ai/mcp/server_status",
            json!({
                "sessionId": "session-0",
                "name": "server-0",
                "status": "ready",
                "tools": [{}, {}, {}]
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::McpChanged(McpUpdate::ServerStatus {
            session_id,
            name,
            tool_count,
            ..
        }) = status
        else {
            panic!("expected MCP server status");
        };
        assert_at_most(&session_id, MAX_IDENTIFIER_BYTES);
        assert_at_most(&name, MAX_IDENTIFIER_BYTES);
        assert_eq!(tool_count, Some(3));
    }

    #[test]
    fn mcp_init_progress_rejects_connected_above_total_and_accepts_equality() {
        let invalid = normalize_grok_extension(
            "_x.ai/mcp/init_progress",
            json!({"sessionId": "s", "total": 1, "connected": 2}),
        )
        .unwrap();
        assert!(matches!(invalid, GrokExtensionOutcome::Malformed(_)));

        let boundary = normalize_grok_extension(
            "_x.ai/mcp/init_progress",
            json!({"sessionId": "s", "total": 2, "connected": 2}),
        )
        .unwrap();
        let GrokExtensionOutcome::McpChanged(McpUpdate::InitProgress {
            total, connected, ..
        }) = boundary
        else {
            panic!("expected valid progress boundary");
        };
        assert_eq!((connected, total), (2, 2));
    }

    #[test]
    fn oversized_identity_and_control_tokens_fail_closed_without_prefix_collisions() {
        let shared_prefix = "i".repeat(MAX_IDENTIFIER_BYTES);
        let first = format!("{shared_prefix}a");
        let second = format!("{shared_prefix}b");

        for candidate in [&first, &second] {
            let outcome = normalize_grok_extension(
                "_x.ai/models/update",
                json!({
                    "currentModelId": candidate,
                    "availableModels": [{"modelId": candidate, "name": "Model"}]
                }),
            )
            .unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
            let serialized = serde_json::to_string(&outcome).unwrap();
            assert!(!serialized.contains(candidate));
            assert!(!serialized.contains(&shared_prefix));
        }

        let oversized_effort = oversized("effort-", MAX_PROTOCOL_TOKEN_LEN);
        let cases = [
            (
                "_x.ai/sessions/changed",
                json!({"upserted": [{"sessionId": first}], "removed": []}),
            ),
            (
                "_x.ai/queue/changed",
                json!({"sessionId": "s", "entries": [{"id": first}]}),
            ),
            (
                "_x.ai/session/prompt_complete",
                json!({
                    "sessionId": "s",
                    "promptId": first,
                    "stopReason": "end_turn"
                }),
            ),
            (
                "_x.ai/mcp/server_status",
                json!({"sessionId": "s", "name": first, "status": "ready"}),
            ),
            (
                "_x.ai/session_notification",
                json!({
                    "sessionId": "s",
                    "update": {
                        "sessionUpdate": "model_changed",
                        "reasoningEffort": oversized_effort
                    }
                }),
            ),
            (
                "_x.ai/announcements/update",
                json!({"gen": 1, "announcements": [{"id": first}]}),
            ),
        ];
        for (method, payload) in cases {
            let outcome = normalize_grok_extension(method, payload).unwrap();
            assert!(
                matches!(outcome, GrokExtensionOutcome::Malformed(_)),
                "{method} accepted an oversized identity/control token"
            );
        }

        let optional = normalize_grok_extension(
            "_x.ai/sessions/changed",
            json!({
                "upserted": [{
                    "sessionId": "s",
                    "modelId": second,
                    "reasoningEffort": oversized("effort-", MAX_PROTOCOL_TOKEN_LEN)
                }],
                "removed": []
            }),
        )
        .unwrap();
        assert!(matches!(optional, GrokExtensionOutcome::Malformed(_)));

        let oversized_url = oversized("https://example.invalid/", MAX_URL_BYTES);
        let announcement = normalize_grok_extension(
            "_x.ai/announcements/update",
            json!({
                "gen": 1,
                "announcements": [{"id": "a", "cta": {"url": oversized_url}}]
            }),
        )
        .unwrap();
        assert!(matches!(announcement, GrokExtensionOutcome::Malformed(_)));
    }

    #[test]
    fn empty_or_control_bearing_actionable_values_are_malformed_and_valid_values_stay_exact() {
        let cases = [
            (
                "_x.ai/models/update",
                json!({
                    "currentModelId": "",
                    "availableModels": [{"modelId": "", "name": "Model"}]
                }),
            ),
            (
                "_x.ai/sessions/changed",
                json!({"upserted": [{"sessionId": "session\nprivate"}], "removed": []}),
            ),
            (
                "_x.ai/queue/changed",
                json!({"sessionId": "s", "entries": [{"id": "\u{0000}"}]}),
            ),
            (
                "_x.ai/session/prompt_complete",
                json!({"sessionId": "s", "promptId": "", "stopReason": "end_turn"}),
            ),
            (
                "_x.ai/mcp/server_status",
                json!({"sessionId": "s", "name": "server\u{007f}", "status": "ready"}),
            ),
            (
                "_x.ai/session_notification",
                json!({
                    "sessionId": "s",
                    "update": {"sessionUpdate": "model_changed", "reasoningEffort": ""}
                }),
            ),
            (
                "_x.ai/announcements/update",
                json!({"gen": 1, "announcements": [{"id": "announcement\rprivate"}]}),
            ),
            (
                "_x.ai/settings/update",
                json!({"gate_url": "https://example.invalid/\nprivate"}),
            ),
        ];
        for (method, payload) in cases {
            let outcome = normalize_grok_extension(method, payload).unwrap();
            assert!(
                matches!(outcome, GrokExtensionOutcome::Malformed(_)),
                "{method} accepted an empty or control-bearing actionable value"
            );
        }

        let exact_session_id = "session/α-01";
        let exact_model_id = "model:β";
        let exact_effort = "x-high";
        let outcome = normalize_grok_extension(
            "_x.ai/sessions/changed",
            json!({
                "upserted": [{
                    "sessionId": exact_session_id,
                    "modelId": exact_model_id,
                    "reasoningEffort": exact_effort
                }],
                "removed": []
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SessionsChanged(changes) = outcome else {
            panic!("expected exact valid values");
        };
        assert_eq!(changes.upserted[0].session_id, exact_session_id);
        assert_eq!(
            changes.upserted[0].model_id.as_deref(),
            Some(exact_model_id)
        );
        assert_eq!(
            changes.upserted[0].reasoning_effort.as_deref(),
            Some(exact_effort)
        );
    }

    #[test]
    fn observed_model_state_becomes_typed_without_retaining_unknown_values() {
        let fake_api_key = ["xai", "-1234567890abcdefghijkl"].concat();
        let outcome = normalize_grok_extension(
            "_x.ai/models/update",
            json!({
                "currentModelId": "grok-build",
                "availableModels": [{
                    "modelId": "grok-build",
                    "name": "Grok Build",
                    "description": "Coding model",
                    "_meta": {
                        "agentType": "build",
                        "reasoningEffort": "xhigh",
                        "reasoningEfforts": [{
                            "id": "deep",
                            "label": "Deep",
                            "description": "More thought",
                            "value": "xhigh",
                            "default": true
                        }],
                        "supportsReasoningEffort": true,
                        "totalContextTokens": 256000,
                        "futureCapability": {"opaque": "discard-me"},
                        "apiKey": fake_api_key.clone()
                    }
                }],
                "futureRoot": "discard-me-too"
            }),
        )
        .unwrap();

        let GrokExtensionOutcome::ModelsUpdated(catalog) = &outcome else {
            panic!("expected models update");
        };
        assert_eq!(catalog.current_model_id, "grok-build");
        assert_eq!(
            catalog.available_models[0].agent_type.as_deref(),
            Some("build")
        );
        assert_eq!(
            catalog.available_models[0].reasoning_efforts[0].value,
            "xhigh"
        );
        assert_eq!(
            catalog.available_models[0].total_context_tokens,
            Some(256000)
        );
        assert!(
            catalog.available_models[0]
                .metadata_keys
                .as_slice()
                .contains(&UNKNOWN_METADATA_KEY.to_owned())
        );
        assert!(
            !catalog.available_models[0]
                .metadata_keys
                .as_slice()
                .contains(&"futureCapability".to_owned())
        );
        assert!(
            !catalog.available_models[0]
                .metadata_keys
                .as_slice()
                .contains(&"apiKey".to_owned())
        );

        let serialized = serde_json::to_string(&outcome).unwrap();
        assert!(!serialized.contains("discard-me"));
        assert!(!serialized.contains(&fake_api_key));
        assert!(!serialized.contains("apiKey"));
    }

    #[test]
    fn display_projection_sanitizes_credentials_paths_and_display_controls() {
        let provider_key = ["xai", "-0123456789abcdef0123456789"].concat();
        let bearer = ["Bearer", " abcdefghijklmnopqrstuvwxyz012345"].concat();
        let windows_path = "C:\\Users\\Private\\project\\secret.txt";
        let unc_path = "\\\\server\\share\\private\\secret.txt";
        let unix_path = "/home/private/project/secret.txt";
        let controlled = "notice\u{202e}hidden\u{0000}\u{001b}";

        let model = normalize_grok_extension(
            "_x.ai/models/update",
            json!({
                "currentModelId": "model",
                "availableModels": [{
                    "modelId": "model",
                    "name": provider_key.clone(),
                    "description": windows_path,
                    "_meta": {
                        "reasoningEffort": "high",
                        "reasoningEfforts": [{
                            "id": "high",
                            "label": bearer.clone(),
                            "description": unix_path,
                            "value": "high"
                        }]
                    }
                }]
            }),
        )
        .unwrap();
        assert!(matches!(model, GrokExtensionOutcome::ModelsUpdated(_)));

        let sessions = normalize_grok_extension(
            "_x.ai/sessions/changed",
            json!({"upserted": [{"sessionId": "s", "title": unc_path}], "removed": []}),
        )
        .unwrap();
        assert!(matches!(sessions, GrokExtensionOutcome::SessionsChanged(_)));

        let mcp = normalize_grok_extension(
            "_x.ai/mcp/servers_updated",
            json!({"mcpServers": [{"name": "server", "displayName": bearer.clone()}]}),
        )
        .unwrap();
        assert!(matches!(mcp, GrokExtensionOutcome::McpChanged(_)));

        let announcements = normalize_grok_extension(
            "_x.ai/announcements/update",
            json!({
                "gen": 1,
                "announcements": [{
                    "id": "a",
                    "title": provider_key.clone(),
                    "message": unix_path,
                    "severity": provider_key.clone(),
                    "cta": {
                        "label": bearer.clone(),
                        "url": "https://example.invalid/action",
                        "caption": unc_path
                    }
                }, {"id": "b", "title": controlled}]
            }),
        )
        .unwrap();
        assert!(matches!(
            announcements,
            GrokExtensionOutcome::AnnouncementsUpdated(_)
        ));

        let settings = normalize_grok_extension(
            "_x.ai/settings/update",
            json!({
                "gate_message": windows_path,
                "gate_label": provider_key.clone(),
                "gate_url": "https://example.invalid/access"
            }),
        )
        .unwrap();
        assert!(matches!(settings, GrokExtensionOutcome::SettingsUpdated(_)));

        let status = normalize_grok_extension(
            "_x.ai/session_notification",
            json!({
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "session_status",
                    "model": {"id": "model", "displayName": unix_path},
                    "errorKind": provider_key.clone()
                }
            }),
        )
        .unwrap();
        assert!(matches!(status, GrokExtensionOutcome::SessionUpdated(_)));

        for outcome in [model, sessions, mcp, announcements, settings, status] {
            let serialized = serde_json::to_string(&outcome).unwrap();
            for private in [
                provider_key.as_str(),
                bearer.as_str(),
                windows_path,
                unc_path,
                unix_path,
                controlled,
            ] {
                assert!(!serialized.contains(private));
            }
            assert!(!serialized.contains('\u{202e}'));
            assert!(!serialized.contains("\\u0000"));
            assert!(!serialized.contains("\\u001b"));
        }
    }

    #[test]
    fn actionable_projection_rejects_credentials_paths_and_controls_without_echoing_them() {
        let provider_key = ["xai", "-0123456789abcdef0123456789"].concat();
        let bearer = ["Bearer", " abcdefghijklmnopqrstuvwxyz012345"].concat();
        let windows_path = "C:\\Users\\Private\\project\\session.json";
        let unc_path = "\\\\server\\share\\private\\session.json";
        let unix_path = "/home/private/project/session.json";
        let controlled = "queue\u{202e}private";
        let cases = [
            (
                "_x.ai/models/update",
                json!({
                    "currentModelId": provider_key.clone(),
                    "availableModels": [{"modelId": provider_key.clone(), "name": "Model"}]
                }),
                provider_key.as_str(),
            ),
            (
                "_x.ai/sessions/changed",
                json!({"upserted": [{"sessionId": windows_path}], "removed": []}),
                windows_path,
            ),
            (
                "_x.ai/queue/changed",
                json!({"sessionId": "s", "entries": [{"id": bearer.clone()}]}),
                bearer.as_str(),
            ),
            (
                "_x.ai/queue/changed",
                json!({"sessionId": "s", "entries": [{"id": controlled}]}),
                controlled,
            ),
            (
                "_x.ai/mcp/server_status",
                json!({"sessionId": "s", "name": unix_path, "status": "ready"}),
                unix_path,
            ),
            (
                "_x.ai/announcements/update",
                json!({"gen": 1, "announcements": [{"id": unc_path}]}),
                unc_path,
            ),
            (
                "_x.ai/session_notification",
                json!({
                    "sessionId": "s",
                    "update": {"sessionUpdate": "model_changed", "modelId": provider_key.clone()}
                }),
                provider_key.as_str(),
            ),
        ];

        for (method, payload, private) in cases {
            let outcome = normalize_grok_extension(method, payload).unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
            assert!(!serde_json::to_string(&outcome).unwrap().contains(private));
        }

        let unsafe_url = format!("https://example.invalid/access/{}", provider_key);
        let outcome = normalize_grok_extension(
            "_x.ai/settings/update",
            json!({"gate_url": unsafe_url.clone()}),
        )
        .unwrap();
        assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        assert!(
            !serde_json::to_string(&outcome)
                .unwrap()
                .contains(&unsafe_url)
        );
    }

    #[test]
    fn every_observed_required_method_has_strict_typed_dispatch() {
        let cases = [
            (
                "_x.ai/announcements/update",
                json!({"gen": 1, "announcements": []}),
            ),
            (
                "_x.ai/mcp/init_progress",
                json!({"total": 1, "connected": 0}),
            ),
            (
                "_x.ai/mcp/server_status",
                json!({
                    "sessionId": "s", "name": "server", "status": "ready"
                }),
            ),
            ("_x.ai/mcp/servers_updated", json!({"mcpServers": []})),
            ("_x.ai/mcp_initialized", json!({"sessionId": "s"})),
            (
                "_x.ai/models/update",
                json!({
                    "currentModelId": "model",
                    "availableModels": [{"modelId": "model", "name": "Model"}]
                }),
            ),
            (
                "_x.ai/queue/changed",
                json!({"sessionId": "s", "entries": []}),
            ),
            (
                "_x.ai/session/prompt_complete",
                json!({
                    "sessionId": "s", "promptId": "p", "stopReason": "end_turn"
                }),
            ),
            (
                "_x.ai/session/update",
                json!({
                    "sessionId": "s", "update": {"sessionUpdate": "turn_completed"}
                }),
            ),
            (
                "_x.ai/session_notification",
                json!({
                    "sessionId": "s", "update": {"sessionUpdate": "response_completed"}
                }),
            ),
            (
                "_x.ai/sessions/changed",
                json!({"upserted": [], "removed": []}),
            ),
            ("_x.ai/settings/update", json!({})),
        ];

        for (method, payload) in cases {
            let outcome = normalize_grok_extension(method, payload).unwrap();
            assert!(
                !matches!(outcome, GrokExtensionOutcome::Malformed(_)),
                "{method}"
            );
            assert!(
                !matches!(outcome, GrokExtensionOutcome::RuntimeEvent(_)),
                "{method}"
            );
        }

        let case_mismatch = normalize_grok_extension(
            "_x.ai/models/UPDATE",
            json!({"currentModelId": "model", "availableModels": []}),
        )
        .unwrap();
        assert!(matches!(
            case_mismatch,
            GrokExtensionOutcome::RuntimeEvent(_)
        ));
    }

    #[test]
    fn known_optional_fields_across_extension_snapshots_reject_wrong_types() {
        let cases = [
            (
                "_x.ai/settings/update",
                json!({"show_resolved_model": "yes"}),
            ),
            (
                "_x.ai/sessions/changed",
                json!({"upserted": [{"sessionId": "s", "activity": {}}]}),
            ),
            (
                "_x.ai/queue/changed",
                json!({"sessionId": "s", "entries": [], "runningPromptId": 1}),
            ),
            (
                "_x.ai/session/prompt_complete",
                json!({
                    "sessionId": "s",
                    "promptId": "p",
                    "stopReason": "end_turn",
                    "errorKind": false
                }),
            ),
            (
                "_x.ai/announcements/update",
                json!({"gen": 1, "announcements": [{"dismissible": "yes"}]}),
            ),
            (
                "_x.ai/announcements/update",
                json!({"gen": 1, "announcements": [{"cta": []}]}),
            ),
            (
                "_x.ai/mcp/init_progress",
                json!({"total": 1, "connected": 0, "sessionId": false}),
            ),
            (
                "_x.ai/mcp_initialized",
                json!({"sessionId": "s", "elapsedMs": "fast"}),
            ),
            (
                "_x.ai/mcp/server_status",
                json!({"sessionId": "s", "name": "server", "status": "ready", "tools": {}}),
            ),
            (
                "_x.ai/mcp/servers_updated",
                json!({"mcpServers": [{"name": "server", "session": {"tools": {}}}]}),
            ),
        ];

        for (case_index, (method, payload)) in cases.into_iter().enumerate() {
            let outcome = normalize_grok_extension(method, payload).unwrap();
            assert!(
                matches!(outcome, GrokExtensionOutcome::Malformed(_)),
                "case {case_index}: {method} accepted a wrong-typed known optional field"
            );
        }
    }

    #[test]
    fn known_optional_nulls_across_extension_snapshots_remain_absent() {
        let cases = [
            (
                "_x.ai/settings/update",
                json!({"show_resolved_model": null, "gate_url": null}),
            ),
            (
                "_x.ai/sessions/changed",
                json!({"upserted": [{"sessionId": "s", "activity": null}]}),
            ),
            (
                "_x.ai/queue/changed",
                json!({"sessionId": "s", "entries": [], "runningPromptId": null}),
            ),
            (
                "_x.ai/session/prompt_complete",
                json!({
                    "sessionId": "s",
                    "promptId": "p",
                    "stopReason": "end_turn",
                    "errorKind": null
                }),
            ),
            (
                "_x.ai/announcements/update",
                json!({"gen": 1, "announcements": [{"id": null, "cta": null}]}),
            ),
            (
                "_x.ai/mcp/init_progress",
                json!({"total": 1, "connected": 0, "sessionId": null}),
            ),
            (
                "_x.ai/mcp_initialized",
                json!({"sessionId": "s", "elapsedMs": null}),
            ),
            (
                "_x.ai/mcp/server_status",
                json!({"sessionId": "s", "name": "server", "status": "ready", "tools": null}),
            ),
            (
                "_x.ai/mcp/servers_updated",
                json!({"mcpServers": [{"name": "server", "session": null}]}),
            ),
        ];

        for (method, payload) in cases {
            let outcome = normalize_grok_extension(method, payload).unwrap();
            assert!(
                !matches!(outcome, GrokExtensionOutcome::Malformed(_)),
                "{method} rejected an allowed optional null"
            );
        }
    }

    #[test]
    fn wire_wrapper_and_sdk_spelling_normalize_to_the_same_outcome() {
        let direct = normalize_grok_extension(
            "x.ai/mcp/init_progress",
            json!({"sessionId": "s", "total": 4, "connected": 2}),
        )
        .unwrap();
        let wrapped = normalize_grok_extension(
            "_x.ai/mcp/init_progress",
            json!({
                "jsonrpc": "2.0",
                "method": "x.ai/mcp/init_progress",
                "params": {"sessionId": "s", "total": 4, "connected": 2}
            }),
        )
        .unwrap();

        assert_eq!(direct, wrapped);
    }

    #[test]
    fn envelope_looking_payloads_are_atomic_and_fail_closed() {
        let malformed_envelopes = [
            json!({
                "jsonrpc": "2.0",
                "method": "x.ai/models/update",
                "params": {},
                "show_resolved_model": true
            }),
            json!({
                "jsonrpc": "2.0",
                "params": {},
                "show_resolved_model": true
            }),
            json!({
                "jsonrpc": "1.0",
                "method": "x.ai/settings/update",
                "params": {"show_resolved_model": true}
            }),
            json!({
                "method": "x.ai/settings/update",
                "params": {"show_resolved_model": true}
            }),
            json!({
                "jsonrpc": "2.0",
                "method": "x.ai/settings/update",
                "params": {"show_resolved_model": true},
                "id": 7
            }),
            json!({
                "jsonrpc": "2.0",
                "method": "x.ai/settings/update",
                "params": null,
                "show_resolved_model": true
            }),
        ];

        for payload in malformed_envelopes {
            let outcome = normalize_grok_extension("_x.ai/settings/update", payload).unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        }

        let bare = normalize_grok_extension(
            "_x.ai/settings/update",
            json!({"show_resolved_model": true}),
        )
        .unwrap();
        assert!(matches!(bare, GrokExtensionOutcome::SettingsUpdated(_)));
    }

    #[test]
    fn unknown_method_collapses_name_and_payload_even_when_both_are_private() {
        let fake_api_key = ["xai", "-1234567890abcdefghijkl"].concat();
        let private_method = format!("_x.ai/future/C:/private/{fake_api_key}");
        let outcome = normalize_grok_extension(
            &private_method,
            json!({
                "sessionId": "must-not-survive",
                "apiKey": fake_api_key.clone(),
                "privateMessage": "must-not-survive"
            }),
        )
        .unwrap();

        let GrokExtensionOutcome::RuntimeEvent(RuntimeEvent::ExtensionMethodObserved {
            session_id,
            method,
        }) = &outcome
        else {
            panic!("expected method-only runtime event");
        };
        assert_eq!(session_id, &None);
        assert_eq!(method.as_str(), UNKNOWN_EXTENSION_METHOD);

        let serialized = serde_json::to_string(&outcome).unwrap();
        assert!(!serialized.contains("must-not-survive"));
        assert!(!serialized.contains(&fake_api_key));
        assert!(!serialized.contains("C:/private"));
    }

    #[test]
    fn malformed_known_payload_keeps_only_safe_key_names() {
        let outcome = normalize_grok_extension(
            "_x.ai/queue/changed",
            json!({
                "entries": "wrong-shape",
                "api_key": "private-value",
                "safeFutureKey": true
            }),
        )
        .unwrap();

        let GrokExtensionOutcome::Malformed(malformed) = &outcome else {
            panic!("expected malformed outcome");
        };
        assert_eq!(malformed.method.as_str(), "_x.ai/queue/changed");
        assert_eq!(
            malformed.payload_keys.as_slice(),
            &[UNKNOWN_METADATA_KEY.to_owned(), "entries".to_owned()]
        );
        let serialized = serde_json::to_string(&outcome).unwrap();
        assert!(!serialized.contains("wrong-shape"));
        assert!(!serialized.contains("private-value"));
        assert!(!serialized.contains("api_key"));
        assert!(!serialized.contains("safeFutureKey"));
    }

    #[test]
    fn metadata_summaries_never_retain_attacker_controlled_object_keys() {
        let alphanumeric_secret = ["xai", "1234567890abcdefghijklmnop"].concat();
        let uuid = "e06a5bf1-9ffc-4c62-bce1-fd90d3018144";
        let path_like = "C.Users.ExampleUser.private.session";
        let windows_path = "C:\\Users\\ExampleUser\\private\\session";
        let outcome = normalize_grok_extension(
            "_x.ai/queue/changed",
            json!({
                "entries": "wrong-shape",
                (alphanumeric_secret.clone()): true,
                (uuid): true,
                (path_like): true,
                (windows_path): true
            }),
        )
        .unwrap();

        let GrokExtensionOutcome::Malformed(malformed) = &outcome else {
            panic!("expected malformed outcome");
        };
        assert_eq!(
            malformed.payload_keys.as_slice(),
            &[UNKNOWN_METADATA_KEY.to_owned(), "entries".to_owned()]
        );

        let serialized = serde_json::to_string(&outcome).unwrap();
        for attacker_key in [alphanumeric_secret.as_str(), uuid, path_like, windows_path] {
            assert!(!serialized.contains(attacker_key));
        }
        assert_eq!(serialized.matches(UNKNOWN_METADATA_KEY).count(), 1);
    }

    #[test]
    fn metadata_key_summary_marks_truncation_at_the_reviewed_key_limit() {
        let reviewed_keys = [
            "_meta",
            "method",
            "params",
            "sessionId",
            "session_id",
            "promptId",
            "prompt_id",
            "update",
            "isReplay",
            "eventId",
            "currentModelId",
            "availableModels",
            "modelId",
            "model_id",
            "name",
            "description",
            "agentType",
            "reasoningEffort",
            "reasoning_effort",
            "reasoningEfforts",
            "supportsReasoningEffort",
            "totalContextTokens",
            "permission_mode",
            "auto_permission_mode_enabled",
            "show_resolved_model",
            "voice_mode_enabled",
            "sharing_enabled",
            "privacy_notice_rollout",
            "privacy_banner_reshow_days",
            "session_picker_grouped",
            "group_tool_verbs",
            "collapsed_edit_blocks",
            "entries",
        ];
        assert_eq!(reviewed_keys.len(), MAX_SUMMARY_KEYS + 1);

        for (count, must_be_truncated) in [(MAX_SUMMARY_KEYS, false), (MAX_SUMMARY_KEYS + 1, true)]
        {
            let payload = Value::Object(
                reviewed_keys[..count]
                    .iter()
                    .map(|key| ((*key).to_owned(), Value::Null))
                    .collect(),
            );
            let outcome = normalize_grok_extension("_x.ai/queue/changed", payload).unwrap();
            let GrokExtensionOutcome::Malformed(malformed) = outcome else {
                panic!("expected malformed outcome");
            };

            assert_eq!(
                malformed
                    .payload_keys
                    .as_slice()
                    .contains(&TRUNCATED_METADATA_KEY.to_owned()),
                must_be_truncated
            );
            assert!(malformed.payload_keys.as_slice().len() <= MAX_SUMMARY_KEYS);
        }
    }

    #[test]
    fn queue_normalization_drops_prompt_text_and_client_attribution() {
        let outcome = normalize_grok_extension(
            "_x.ai/queue/changed",
            json!({
                "sessionId": "s",
                "entries": [{
                    "id": "p1",
                    "version": 3,
                    "owner": "private-client",
                    "lastEditor": "private-editor",
                    "kind": "prompt",
                    "text": "private queued prompt",
                    "position": 0,
                    "combinedTexts": ["private queued prompt"]
                }],
                "runningPromptId": "p0",
                "runningText": "private running prompt"
            }),
        )
        .unwrap();

        let GrokExtensionOutcome::QueueChanged(queue) = &outcome else {
            panic!("expected queue update");
        };
        assert_eq!(queue.entries[0].kind, QueueEntryKind::Prompt);
        assert_eq!(queue.running_prompt_id.as_deref(), Some("p0"));
        let serialized = serde_json::to_string(&outcome).unwrap();
        assert!(!serialized.contains("private queued prompt"));
        assert!(!serialized.contains("private running prompt"));
        assert!(!serialized.contains("private-client"));
    }

    #[test]
    fn mcp_summary_omits_transport_credentials_and_discards_diagnostic_content() {
        let outcome = normalize_grok_extension(
            "_x.ai/mcp/servers_updated",
            json!({
                "mcpServers": [{
                    "name": "local-server",
                    "displayName": "Local server",
                    "source": "local",
                    "type": "stdio",
                    "command": "private-command",
                    "args": ["--token", "private-argument"],
                    "env": [{"name": "API_KEY", "value": "private-secret"}],
                    "session": {
                        "enabled": true,
                        "status": "ready",
                        "tools": [{"name": "one"}, {"name": "two"}]
                    }
                }]
            }),
        )
        .unwrap();
        let serialized = serde_json::to_string(&outcome).unwrap();
        assert!(serialized.contains("local-server"));
        assert!(serialized.contains("\"tool_count\":2"));
        assert!(!serialized.contains("private-command"));
        assert!(!serialized.contains("private-argument"));
        assert!(!serialized.contains("private-secret"));

        let status = normalize_grok_extension(
            "_x.ai/mcp/server_status",
            json!({
                "sessionId": "s",
                "name": "local-server",
                "source": "local",
                "status": "unavailable",
                "reason": "handshake_failed",
                "detail": "api_key=private-secret"
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::McpChanged(McpUpdate::ServerStatus { detail_present, .. }) =
            &status
        else {
            panic!("expected MCP server status");
        };
        assert!(*detail_present);
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(serialized.contains("\"detail_present\":true"));
        assert!(!serialized.contains("private-secret"));
    }

    #[test]
    fn mcp_status_detail_is_presence_only_for_arbitrary_text_and_objects() {
        let sensitive_path = "D:/Clients/Private Project/credentials.txt";
        let secret_key = "plainAlphanumericSecret123";
        let uuid_key = "e06a5bf1-9ffc-4c62-bce1-fd90d3018144";
        let path_key = "C:/Users/ExampleUser/private/session";
        let details = [
            json!(format!("failed while reading {sensitive_path}")),
            json!({
                (secret_key): "private-value",
                (uuid_key): {"nestedPrivateKey": "nested-private-value"},
                (path_key): ["private-array-value"]
            }),
            Value::Null,
        ];

        for detail in details {
            let outcome = normalize_grok_extension(
                "_x.ai/mcp/server_status",
                json!({
                    "sessionId": "s",
                    "name": "server",
                    "status": "unavailable",
                    "detail": detail
                }),
            )
            .unwrap();
            let GrokExtensionOutcome::McpChanged(McpUpdate::ServerStatus {
                detail_present, ..
            }) = &outcome
            else {
                panic!("expected MCP server status");
            };
            assert!(*detail_present);

            let serialized = serde_json::to_string(&outcome).unwrap();
            assert!(serialized.contains("\"detail_present\":true"));
            for forbidden in [
                sensitive_path,
                secret_key,
                uuid_key,
                path_key,
                "private-value",
                "nestedPrivateKey",
                "nested-private-value",
                "private-array-value",
            ] {
                assert!(
                    !serialized.contains(forbidden),
                    "normalized status contains {forbidden}"
                );
            }
        }

        let absent = normalize_grok_extension(
            "_x.ai/mcp/server_status",
            json!({"sessionId": "s", "name": "server", "status": "ready"}),
        )
        .unwrap();
        let GrokExtensionOutcome::McpChanged(McpUpdate::ServerStatus { detail_present, .. }) =
            absent
        else {
            panic!("expected MCP server status");
        };
        assert!(!detail_present);
    }

    #[test]
    fn settings_preserve_null_semantics_without_account_payloads() {
        let cleared = normalize_grok_extension(
            "_x.ai/settings/update",
            json!({
                "permission_mode": null,
                "auto_permission_mode_enabled": true,
                "subscription_tier_display": "private-tier",
                "campaigns": [{"private": "private-campaign-body"}]
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SettingsUpdated(settings) = &cleared else {
            panic!("expected settings");
        };
        assert_eq!(settings.permission_mode, OptionalSetting::Cleared);
        assert_eq!(settings.auto_permission_mode_enabled, Some(true));
        let serialized = serde_json::to_string(&cleared).unwrap();
        assert!(!serialized.contains("private-tier"));
        assert!(!serialized.contains("private-campaign-body"));

        let omitted = normalize_grok_extension("x.ai/settings/update", json!({})).unwrap();
        let GrokExtensionOutcome::SettingsUpdated(settings) = omitted else {
            panic!("expected settings");
        };
        assert_eq!(settings.permission_mode, OptionalSetting::NotReported);

        let populated = normalize_grok_extension(
            "_x.ai/settings/update",
            json!({
                "permission_mode": "always-approve",
                "sharing_enabled": true,
                "privacy_notice_rollout": false,
                "privacy_banner_reshow_days": 30,
                "session_picker_grouped": true,
                "group_tool_verbs": true,
                "collapsed_edit_blocks": false,
                "subscription_watch_interval_secs": 120
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SettingsUpdated(settings) = populated else {
            panic!("expected settings");
        };
        assert_eq!(
            settings.permission_mode,
            OptionalSetting::Value(PermissionMode::AlwaysApprove)
        );
        assert_eq!(settings.sharing_enabled, Some(true));
        assert_eq!(settings.privacy_banner_reshow_days, Some(30));
        assert_eq!(settings.subscription_watch_interval_secs, Some(120));
    }

    #[test]
    fn session_update_optional_fields_fail_closed_on_wrong_types_and_alias_conflicts() {
        let common_updates = [
            json!({"sessionUpdate": "turn_completed", "promptId": 7}),
            json!({"sessionUpdate": "turn_completed", "modelId": false}),
            json!({"sessionUpdate": "turn_completed", "reasoningEffort": []}),
            json!({"sessionUpdate": "turn_completed", "stopReason": {}}),
            json!({"sessionUpdate": "turn_completed", "errorKind": []}),
            json!({"sessionUpdate": "turn_completed", "elapsedMs": "42"}),
            json!({"sessionUpdate": "turn_completed", "usage": []}),
            json!({
                "sessionUpdate": "turn_completed",
                "usage": {"inputTokens": "10"}
            }),
            json!({
                "sessionUpdate": "turn_completed",
                "promptId": "one",
                "prompt_id": "two"
            }),
            json!({
                "sessionUpdate": "turn_completed",
                "usage": {"inputTokens": 1, "input_tokens": 1}
            }),
        ];
        for update in common_updates {
            let outcome = normalize_grok_extension(
                "_x.ai/session/update",
                json!({"sessionId": "s", "update": update}),
            )
            .unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        }

        for metadata in [json!([]), json!({"isReplay": "true"})] {
            let outcome = normalize_grok_extension(
                "_x.ai/session_notification",
                json!({
                    "sessionId": "s",
                    "update": {"sessionUpdate": "response_completed"},
                    "_meta": metadata
                }),
            )
            .unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        }

        let status_updates = [
            json!({"sessionUpdate": "session_status", "model": []}),
            json!({"sessionUpdate": "session_status", "model": {"id": false}}),
            json!({"sessionUpdate": "session_status", "model": {"displayName": 7}}),
            json!({"sessionUpdate": "session_status", "effort": "high"}),
            json!({"sessionUpdate": "session_status", "effort": {"level": false}}),
            json!({"sessionUpdate": "session_status", "contextWindow": []}),
            json!({
                "sessionUpdate": "session_status",
                "contextWindow": {"contextWindowSize": "large"}
            }),
            json!({
                "sessionUpdate": "session_status",
                "contextWindow": {"usedPercentage": 256}
            }),
            json!({
                "sessionUpdate": "session_status",
                "contextWindow": {"sessionUsage": []}
            }),
            json!({
                "sessionUpdate": "session_status",
                "contextWindow": {"sessionUsage": {"outputTokens": false}}
            }),
            json!({
                "sessionUpdate": "session_status",
                "contextWindow": {
                    "sessionUsage": {"inputTokens": 1},
                    "sessionInputTokens": "bad"
                }
            }),
            json!({"sessionUpdate": "session_status", "turn": []}),
            json!({
                "sessionUpdate": "session_status",
                "turn": {"startedAtMs": "now"}
            }),
            json!({"sessionUpdate": "session_status", "trigger": true}),
            json!({
                "sessionUpdate": "session_status",
                "contextWindow": {},
                "context_window": {}
            }),
        ];
        for update in status_updates {
            let outcome = normalize_grok_extension(
                "_x.ai/session_notification",
                json!({"sessionId": "s", "update": update}),
            )
            .unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        }
    }

    #[test]
    fn session_update_optional_nulls_preserve_absence_semantics() {
        let outcome = normalize_grok_extension(
            "_x.ai/session_notification",
            json!({
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "session_status",
                    "promptId": null,
                    "stopReason": null,
                    "errorKind": null,
                    "elapsedMs": null,
                    "modelId": null,
                    "reasoningEffort": null,
                    "usage": null,
                    "schemaVersion": null,
                    "model": null,
                    "effort": null,
                    "contextWindow": null,
                    "turn": null,
                    "trigger": null
                },
                "_meta": {"isReplay": null}
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SessionUpdated(update) = outcome else {
            panic!("optional nulls should normalize as absent");
        };
        assert_eq!(update.prompt_id, None);
        assert_eq!(update.usage, None);
        assert!(!update.replay);
        let status = update.status.expect("session status snapshot");
        assert_eq!(status.schema_version, None);
        assert_eq!(status.model_id, None);
        assert_eq!(status.context_window_tokens, None);
        assert_eq!(status.trigger, None);
    }

    #[test]
    fn session_status_percentages_are_bounded_to_one_hundred() {
        let boundary = normalize_grok_extension(
            "_x.ai/session_notification",
            json!({
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "session_status",
                    "contextWindow": {
                        "usedPercentage": 100,
                        "remainingPercentage": 100,
                        "autoCompactThresholdPercent": 100
                    }
                }
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SessionUpdated(update) = boundary else {
            panic!("expected valid percentage boundary");
        };
        let status = update.status.expect("status snapshot");
        assert_eq!(status.used_percentage, Some(100));
        assert_eq!(status.remaining_percentage, Some(100));
        assert_eq!(status.auto_compact_threshold_percentage, Some(100));

        for field in [
            "usedPercentage",
            "remainingPercentage",
            "autoCompactThresholdPercent",
        ] {
            let mut context = serde_json::Map::new();
            context.insert(field.to_owned(), json!(101));
            let outcome = normalize_grok_extension(
                "_x.ai/session_notification",
                json!({
                    "sessionId": "s",
                    "update": {
                        "sessionUpdate": "session_status",
                        "contextWindow": context
                    }
                }),
            )
            .unwrap();
            assert!(matches!(outcome, GrokExtensionOutcome::Malformed(_)));
        }
    }

    #[test]
    fn session_status_usage_requires_nested_and_flattened_values_to_agree() {
        let conflict = normalize_grok_extension(
            "_x.ai/session_notification",
            json!({
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "session_status",
                    "contextWindow": {
                        "sessionUsage": {"inputTokens": 10, "outputTokens": 4},
                        "sessionInputTokens": 11,
                        "sessionOutputTokens": 4
                    }
                }
            }),
        )
        .unwrap();
        assert!(matches!(conflict, GrokExtensionOutcome::Malformed(_)));

        for context_window in [
            json!({
                "sessionUsage": {"inputTokens": 10, "outputTokens": 4},
                "sessionInputTokens": 10,
                "sessionOutputTokens": 4
            }),
            json!({
                "sessionUsage": {"inputTokens": 10},
                "sessionOutputTokens": 4
            }),
            json!({
                "sessionUsage": null,
                "sessionInputTokens": 10,
                "sessionOutputTokens": 4
            }),
        ] {
            let outcome = normalize_grok_extension(
                "_x.ai/session_notification",
                json!({
                    "sessionId": "s",
                    "update": {
                        "sessionUpdate": "session_status",
                        "contextWindow": context_window
                    }
                }),
            )
            .unwrap();
            let GrokExtensionOutcome::SessionUpdated(update) = outcome else {
                panic!("equal or complementary usage representations must be accepted");
            };
            let usage = update
                .status
                .and_then(|status| status.usage)
                .expect("usage snapshot");
            assert_eq!(usage.input_tokens, Some(10));
            assert_eq!(usage.output_tokens, Some(4));
        }
    }

    #[test]
    fn session_updates_project_usage_and_status_without_private_status_fields() {
        let usage = normalize_grok_extension(
            "_x.ai/session/update",
            json!({
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "turn_completed",
                    "prompt_id": "p",
                    "stop_reason": "end_turn",
                    "error_kind": "safe_error",
                    "elapsed_ms": 42,
                    "model_id": "grok-build",
                    "reasoning_effort": "high",
                    "usage": {
                        "input_tokens": 10,
                        "outputTokens": 4,
                        "total_tokens": 14,
                        "cachedReadTokens": 3,
                        "cache_creation_input_tokens": 2,
                        "reasoning_tokens": 1,
                        "modelCalls": 2,
                        "api_duration_ms": 21,
                        "numTurns": 1,
                        "usage_is_incomplete": false,
                        "costUsdTicks": 999999,
                        "modelUsage": {"private-model-row": {"costUSD": 50}}
                    },
                    "agent_result": "private turn result"
                }
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SessionUpdated(update) = &usage else {
            panic!("expected session update");
        };
        let usage = update.usage.expect("usage must be projected");
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(4));
        assert_eq!(usage.cached_read_tokens, Some(3));
        assert_eq!(usage.cache_creation_tokens, Some(2));
        assert_eq!(usage.incomplete, Some(false));
        assert_eq!(update.error_kind.as_deref(), Some("safe_error"));
        assert_eq!(update.elapsed_ms, Some(42));
        let serialized = serde_json::to_string(&usage).unwrap();
        assert!(!serialized.contains("private-model-row"));
        assert!(!serialized.contains("cost"));
        assert!(
            !serde_json::to_string(&update)
                .unwrap()
                .contains("private turn result")
        );

        let status = normalize_grok_extension(
            "_x.ai/session_notification",
            json!({
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "session_status",
                    "schema_version": 1,
                    "cwd": "C:/private/workspace",
                    "transcript_path": "C:/private/transcript.jsonl",
                    "model": {"id": "grok-build", "display_name": "Grok Build"},
                    "effort": {"level": "high"},
                    "workspace": {
                        "repo_root": "C:/private/workspace",
                        "repo": {"host": "private-host", "owner": "private-owner"}
                    },
                    "cost": {"total_cost_usd": 1.25},
                    "context_window": {
                        "context_window_size": 256000,
                        "context_tokens": 1200,
                        "session_usage": {
                            "input_tokens": 100,
                            "output_tokens": 25,
                            "cache_read_input_tokens": 50,
                            "cache_creation_input_tokens": 5
                        },
                        "used_percentage": 20,
                        "remaining_percentage": 80,
                        "auto_compact_threshold_percent": 82
                    },
                    "turn": {"started_at_ms": 1234},
                    "trigger": "refresh_interval"
                }
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SessionUpdated(update) = &status else {
            panic!("expected status update");
        };
        let status = update.status.as_ref().expect("status must be projected");
        assert_eq!(status.schema_version, Some(1));
        assert_eq!(status.model_id.as_deref(), Some("grok-build"));
        assert_eq!(status.context_window_tokens, Some(256000));
        assert_eq!(
            status.usage.expect("status usage").cached_read_tokens,
            Some(50)
        );
        assert_eq!(status.trigger, Some(SessionStatusTrigger::RefreshInterval));
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("C:/private"));
        assert!(!serialized.contains("private-host"));
        assert!(!serialized.contains("private-owner"));
        assert!(!serialized.contains("total_cost_usd"));
    }

    #[test]
    fn session_and_prompt_outcomes_omit_free_form_result_bodies() {
        let prompt = normalize_grok_extension(
            "_x.ai/session/prompt_complete",
            json!({
                "sessionId": "s",
                "promptId": "p",
                "stopReason": "error",
                "errorKind": "max_tokens_truncation",
                "agentResult": "private upstream response"
            }),
        )
        .unwrap();
        let serialized = serde_json::to_string(&prompt).unwrap();
        assert!(!serialized.contains("private upstream response"));

        let session = normalize_grok_extension(
            "_x.ai/session_notification",
            json!({
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "session_summary_generated",
                    "session_summary": "private prompt-derived title",
                    "futureField": "private update value"
                },
                "_meta": {
                    "isReplay": true,
                    "eventId": "s-7",
                    "accessToken": "private-token"
                }
            }),
        )
        .unwrap();
        let GrokExtensionOutcome::SessionUpdated(update) = &session else {
            panic!("expected session update");
        };
        assert!(update.replay);
        assert_eq!(update.kind, SessionUpdateKind::SessionSummaryGenerated);
        assert!(
            !update
                .metadata_keys
                .as_slice()
                .contains(&"accessToken".to_owned())
        );
        let serialized = serde_json::to_string(&session).unwrap();
        assert!(!serialized.contains("private prompt-derived title"));
        assert!(!serialized.contains("private update value"));
        assert!(!serialized.contains("private-token"));
    }
}
