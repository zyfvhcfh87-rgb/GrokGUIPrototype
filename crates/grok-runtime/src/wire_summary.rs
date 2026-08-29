use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use agent_client_protocol::LineDirection;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::ExtensionMethod;

const MAX_RECORDED_FRAMES: usize = 1_024;
const MAX_BATCH_ITEMS: usize = 128;
const MAX_ARRAY_ITEMS_PER_FIELD: usize = 64;
const MAX_OBJECT_FIELDS_PER_NODE: usize = 256;
const MAX_FIELD_PATHS_PER_FRAME: usize = 64;
const MAX_FIELD_DEPTH: usize = 8;
const MAX_FIELD_SEGMENT_BYTES: usize = 64;
const MAX_REQUEST_ID_BYTES: usize = 128;
pub(crate) const MAX_WIRE_LINE_BYTES: usize = 2 * 1024 * 1024;
const UNKNOWN_FIELD_SEGMENT: &str = "<unknown-key>";
const UNKNOWN_METHOD: &str = "<unknown-method>";
const INVALID_METHOD: &str = "<invalid-method>";
const UNTRACKED_REQUEST_ID: &str = "<untracked-request>";
const DUPLICATE_REQUEST_ID: &str = "<duplicate-request>";

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireSummary {
    pub client_methods: BTreeSet<String>,
    pub agent_methods: BTreeSet<String>,
    pub client_response_count: u64,
    pub agent_response_count: u64,
    pub malformed_stdout_lines: u64,
    pub stderr_line_count: u64,
    pub stderr_byte_count: u64,
    pub dropped_frame_count: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub untracked_request_id_count: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub duplicate_request_id_count: u64,
    pub frames: Vec<SanitizedWireFrame>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedWireFrame {
    pub sequence: u64,
    pub direction: WireDirection,
    pub kind: WireFrameKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub field_paths: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub field_paths_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_count: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireDirection {
    ClientToAgent,
    AgentToClient,
    AgentDiagnostic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireFrameKind {
    Request,
    Notification,
    Response,
    ErrorResponse,
    Malformed,
    Diagnostic,
    Unknown,
}

/// Shared, privacy-minimized transport observer.
///
/// Raw lines are parsed and reduced while the lock is held; only bounded
/// structural evidence and diagnostic byte counts are retained.
#[derive(Clone, Default)]
pub struct WireCapture(Arc<Mutex<WireRecorder>>);

impl WireCapture {
    pub fn record_transport_line(&self, line: &str, direction: LineDirection) {
        lock_unpoisoned(&self.0).record_line(line, direction);
    }

    pub(crate) fn record_diagnostic_line(&self, byte_count: usize) {
        lock_unpoisoned(&self.0).record_diagnostic_line(byte_count as u64);
    }

    pub(crate) fn record_malformed_transport_line(&self, direction: LineDirection) {
        lock_unpoisoned(&self.0).record_malformed_line(direction, None);
    }

    #[must_use]
    pub fn summary(&self) -> WireSummary {
        lock_unpoisoned(&self.0).summary()
    }
}

impl fmt::Debug for WireCapture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WireCapture")
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
pub(crate) struct WireRecorder {
    summary: WireSummary,
    requests: BTreeMap<RequestIdentity, RequestRecord>,
    next_request_id: u64,
    next_sequence: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RequestIdentity {
    origin: RequestOrigin,
    id: RequestId,
}

/// Exact, bounded request identity retained only inside the live recorder.
///
/// It deliberately has no serialization implementation: public evidence uses
/// only the monotonically assigned `request-NNNN` aliases.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RequestId {
    String(String),
    Number(String),
}

#[derive(Clone, Debug)]
struct RequestRecord {
    mapped_id: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RequestOrigin {
    Client,
    Agent,
}

impl WireRecorder {
    pub(crate) fn record_line(&mut self, line: &str, direction: LineDirection) {
        if direction == LineDirection::Stderr {
            self.record_diagnostic_line(line.len() as u64);
            return;
        }

        if line.len() > MAX_WIRE_LINE_BYTES {
            self.record_malformed_line(direction, Some(line.len() as u64));
            return;
        }

        let Ok(value) = serde_json::from_str::<Value>(line) else {
            self.record_malformed_line(direction, Some(line.len() as u64));
            return;
        };

        self.record_value(&value, direction);
    }

    pub(crate) fn summary(&self) -> WireSummary {
        self.summary.clone()
    }

    fn record_diagnostic_line(&mut self, byte_count: u64) {
        self.summary.stderr_line_count = self.summary.stderr_line_count.saturating_add(1);
        self.summary.stderr_byte_count = self.summary.stderr_byte_count.saturating_add(byte_count);
        let sequence = self.next_sequence();
        self.push_frame(SanitizedWireFrame {
            sequence,
            direction: WireDirection::AgentDiagnostic,
            kind: WireFrameKind::Diagnostic,
            request_id: None,
            method: None,
            field_paths: BTreeSet::new(),
            field_paths_truncated: false,
            byte_count: Some(byte_count),
        });
    }

    fn record_malformed_line(&mut self, direction: LineDirection, byte_count: Option<u64>) {
        if direction == LineDirection::Stdout {
            self.summary.malformed_stdout_lines =
                self.summary.malformed_stdout_lines.saturating_add(1);
        }
        let sequence = self.next_sequence();
        self.push_frame(SanitizedWireFrame {
            sequence,
            direction: wire_direction(direction),
            kind: WireFrameKind::Malformed,
            request_id: None,
            method: None,
            field_paths: BTreeSet::new(),
            field_paths_truncated: false,
            byte_count,
        });
    }

    fn record_value(&mut self, value: &Value, direction: LineDirection) {
        if let Value::Array(values) = value {
            if values.is_empty() {
                self.record_unknown_value(direction);
                return;
            }
            for value in values.iter().take(MAX_BATCH_ITEMS) {
                self.record_batch_item(value, direction);
            }
            self.summary.dropped_frame_count = self
                .summary
                .dropped_frame_count
                .saturating_add(values.len().saturating_sub(MAX_BATCH_ITEMS) as u64);
            return;
        }

        self.record_batch_item(value, direction);
    }

    fn record_batch_item(&mut self, value: &Value, direction: LineDirection) {
        if self.summary.frames.len() >= MAX_RECORDED_FRAMES {
            self.summary.dropped_frame_count = self.summary.dropped_frame_count.saturating_add(1);
            return;
        }

        if value.is_array() {
            self.record_unknown_value(direction);
            return;
        }

        let Some(object) = value.as_object() else {
            self.record_unknown_value(direction);
            return;
        };

        let classification = classify_jsonrpc_frame(object);
        let kind = classification.kind;
        let method = classification.method;

        if matches!(kind, WireFrameKind::Request | WireFrameKind::Notification)
            && let Some(method) = &method
        {
            match direction {
                LineDirection::Stdin => {
                    self.summary.client_methods.insert(method.clone());
                }
                LineDirection::Stdout => {
                    self.summary.agent_methods.insert(method.clone());
                }
                LineDirection::Stderr => unreachable!("stderr is handled before JSON parsing"),
            }
        } else if matches!(kind, WireFrameKind::Response | WireFrameKind::ErrorResponse) {
            match direction {
                LineDirection::Stdin => {
                    self.summary.client_response_count =
                        self.summary.client_response_count.saturating_add(1);
                }
                LineDirection::Stdout => {
                    self.summary.agent_response_count =
                        self.summary.agent_response_count.saturating_add(1);
                }
                LineDirection::Stderr => unreachable!("stderr is handled before JSON parsing"),
            }
        }

        let request = classification.request_id.map(|id| match kind {
            WireFrameKind::Request => self.record_request_id(request_origin(direction), id),
            WireFrameKind::Response | WireFrameKind::ErrorResponse => {
                self.record_response_id(opposite_origin(direction), id)
            }
            WireFrameKind::Notification
            | WireFrameKind::Malformed
            | WireFrameKind::Diagnostic
            | WireFrameKind::Unknown => {
                unreachable!("only valid requests and responses retain request identity")
            }
        });
        let mut field_paths = BTreeSet::new();
        let mut field_paths_truncated = false;
        for root in ["params", "result", "error"] {
            if let Some(value) = object.get(root) {
                if !field_paths.contains(root) && field_paths.len() >= MAX_FIELD_PATHS_PER_FRAME {
                    field_paths_truncated = true;
                    continue;
                }
                field_paths.insert(root.to_owned());
                collect_field_paths(value, root, 0, &mut field_paths, &mut field_paths_truncated);
            }
        }
        let sequence = self.next_sequence();
        self.push_frame(SanitizedWireFrame {
            sequence,
            direction: wire_direction(direction),
            kind,
            request_id: request.map(|request| request.mapped_id),
            method,
            field_paths,
            field_paths_truncated,
            byte_count: None,
        });
    }

    fn record_unknown_value(&mut self, direction: LineDirection) {
        if self.summary.frames.len() >= MAX_RECORDED_FRAMES {
            self.summary.dropped_frame_count = self.summary.dropped_frame_count.saturating_add(1);
            return;
        }
        let sequence = self.next_sequence();
        self.push_frame(SanitizedWireFrame {
            sequence,
            direction: wire_direction(direction),
            kind: WireFrameKind::Unknown,
            request_id: None,
            method: None,
            field_paths: BTreeSet::new(),
            field_paths_truncated: false,
            byte_count: None,
        });
    }

    fn record_request_id(&mut self, origin: RequestOrigin, id: RequestId) -> RequestRecord {
        let identity = RequestIdentity { origin, id };
        if self.requests.contains_key(&identity) {
            self.summary.duplicate_request_id_count =
                self.summary.duplicate_request_id_count.saturating_add(1);
            return RequestRecord {
                mapped_id: DUPLICATE_REQUEST_ID.to_owned(),
            };
        }

        self.next_request_id = self.next_request_id.saturating_add(1);
        let record = RequestRecord {
            mapped_id: format!("request-{:04}", self.next_request_id),
        };
        self.requests.insert(identity, record.clone());
        record
    }

    fn record_response_id(&mut self, origin: RequestOrigin, id: RequestId) -> RequestRecord {
        let identity = RequestIdentity { origin, id };
        self.requests
            .remove(&identity)
            .unwrap_or_else(|| self.untracked_request())
    }

    fn untracked_request(&mut self) -> RequestRecord {
        self.summary.untracked_request_id_count =
            self.summary.untracked_request_id_count.saturating_add(1);
        RequestRecord {
            mapped_id: UNTRACKED_REQUEST_ID.to_owned(),
        }
    }

    fn next_sequence(&mut self) -> u64 {
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.next_sequence
    }

    fn push_frame(&mut self, frame: SanitizedWireFrame) {
        if self.summary.frames.len() < MAX_RECORDED_FRAMES {
            self.summary.frames.push(frame);
        } else {
            self.summary.dropped_frame_count = self.summary.dropped_frame_count.saturating_add(1);
        }
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug)]
struct FrameClassification {
    kind: WireFrameKind,
    method: Option<String>,
    request_id: Option<RequestId>,
}

/// Classify only complete JSON-RPC 2.0 envelope shapes.
///
/// In particular, malformed or ambiguous objects never receive a request
/// identity, so they cannot create, retire, or collide with correlation state.
fn classify_jsonrpc_frame(object: &Map<String, Value>) -> FrameClassification {
    let method = classify_method(object.get("method"));
    let unknown_method = method.as_ref().map(|method| method.display.clone());
    let unknown = || FrameClassification {
        kind: WireFrameKind::Unknown,
        method: unknown_method.clone(),
        request_id: None,
    };

    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return unknown();
    }

    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    match method.as_ref() {
        Some(MethodClassification {
            valid: true,
            display,
        }) if !has_result && !has_error && has_valid_params(object) => {
            if !object.contains_key("id") {
                FrameClassification {
                    kind: WireFrameKind::Notification,
                    method: Some(display.clone()),
                    request_id: None,
                }
            } else if let Some(request_id) = object.get("id").and_then(request_id) {
                FrameClassification {
                    kind: WireFrameKind::Request,
                    method: Some(display.clone()),
                    request_id: Some(request_id),
                }
            } else {
                unknown()
            }
        }
        None if has_result ^ has_error && !object.contains_key("params") => {
            if has_error && !has_valid_error(object) {
                return unknown();
            }
            if let Some(request_id) = object.get("id").and_then(request_id) {
                FrameClassification {
                    kind: if has_error {
                        WireFrameKind::ErrorResponse
                    } else {
                        WireFrameKind::Response
                    },
                    method: None,
                    request_id: Some(request_id),
                }
            } else {
                unknown()
            }
        }
        Some(_) | None => unknown(),
    }
}

fn has_valid_params(object: &Map<String, Value>) -> bool {
    object
        .get("params")
        .is_none_or(|params| params.is_object() || params.is_array())
}

fn has_valid_error(object: &Map<String, Value>) -> bool {
    let Some(error) = object.get("error").and_then(Value::as_object) else {
        return false;
    };
    let integer_code = error
        .get("code")
        .and_then(Value::as_number)
        .is_some_and(|code| code.is_i64() || code.is_u64());
    integer_code && error.get("message").is_some_and(Value::is_string)
}

#[derive(Debug)]
struct MethodClassification {
    valid: bool,
    display: String,
}

fn classify_method(value: Option<&Value>) -> Option<MethodClassification> {
    let value = value?;
    let Some(method) = value.as_str() else {
        return Some(MethodClassification {
            valid: false,
            display: INVALID_METHOD.to_owned(),
        });
    };
    if ExtensionMethod::new(method).is_err() {
        return Some(MethodClassification {
            valid: false,
            display: INVALID_METHOD.to_owned(),
        });
    }

    Some(MethodClassification {
        valid: true,
        display: if is_known_method(method) {
            method.to_owned()
        } else {
            UNKNOWN_METHOD.to_owned()
        },
    })
}

fn request_id(id: &Value) -> Option<RequestId> {
    match id {
        Value::String(value) if value.len() <= MAX_REQUEST_ID_BYTES => {
            Some(RequestId::String(value.to_owned()))
        }
        Value::Number(value) if value.is_i64() || value.is_u64() => {
            let rendered = value.to_string();
            (rendered.len() <= MAX_REQUEST_ID_BYTES).then_some(RequestId::Number(rendered))
        }
        Value::Null
        | Value::Bool(_)
        | Value::Array(_)
        | Value::Object(_)
        | Value::String(_)
        | Value::Number(_) => None,
    }
}

fn wire_direction(direction: LineDirection) -> WireDirection {
    match direction {
        LineDirection::Stdin => WireDirection::ClientToAgent,
        LineDirection::Stdout => WireDirection::AgentToClient,
        LineDirection::Stderr => WireDirection::AgentDiagnostic,
    }
}

fn request_origin(direction: LineDirection) -> RequestOrigin {
    match direction {
        LineDirection::Stdin => RequestOrigin::Client,
        LineDirection::Stdout => RequestOrigin::Agent,
        LineDirection::Stderr => unreachable!("stderr cannot contain a request"),
    }
}

fn opposite_origin(direction: LineDirection) -> RequestOrigin {
    match direction {
        LineDirection::Stdin => RequestOrigin::Agent,
        LineDirection::Stdout => RequestOrigin::Client,
        LineDirection::Stderr => unreachable!("stderr cannot contain a response"),
    }
}

/// ACP methods supported by the v1 SDK plus the Grok methods observed by the
/// checked-in compatibility probes. Unknown but syntactically valid methods
/// are useful only as a presence signal and collapse to one placeholder.
fn is_known_method(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "authenticate"
            | "logout"
            | "session/new"
            | "session/load"
            | "session/list"
            | "session/delete"
            | "session/fork"
            | "session/resume"
            | "session/close"
            | "session/set_mode"
            | "session/set_config_option"
            | "session/set_model"
            | "session/prompt"
            | "session/cancel"
            | "session/update"
            | "session/request_permission"
            | "fs/write_text_file"
            | "fs/read_text_file"
            | "terminal/create"
            | "terminal/output"
            | "terminal/release"
            | "terminal/wait_for_exit"
            | "terminal/kill"
            | "elicitation/create"
            | "elicitation/complete"
            | "mcp/connect"
            | "mcp/message"
            | "mcp/disconnect"
            | "$/cancel_request"
            | "_x.ai/announcements/update"
            | "_x.ai/mcp/init_progress"
            | "_x.ai/mcp/server_status"
            | "_x.ai/mcp/servers_updated"
            | "_x.ai/mcp_initialized"
            | "_x.ai/models/update"
            | "_x.ai/queue/changed"
            | "_x.ai/session/prompt_complete"
            | "_x.ai/session/update"
            | "_x.ai/session_notification"
            | "_x.ai/sessions/changed"
            | "_x.ai/settings/update"
            | "_x.ai/yolo_mode_changed"
            | "x.ai/announcements/update"
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
            | "x.ai/yolo_mode_changed"
    )
}

fn collect_field_paths(
    value: &Value,
    prefix: &str,
    depth: usize,
    fields: &mut BTreeSet<String>,
    truncated: &mut bool,
) {
    if depth >= MAX_FIELD_DEPTH {
        *truncated |= value_has_structural_children(value);
        return;
    }

    match value {
        Value::Object(map) => collect_object_paths(map, prefix, depth, fields, truncated),
        Value::Array(values) => {
            let path = format!("{prefix}[]");
            if !fields.contains(&path) && fields.len() >= MAX_FIELD_PATHS_PER_FRAME {
                // The array container is itself shape evidence, even when empty.
                *truncated = true;
                return;
            }
            fields.insert(path.clone());
            for (index, child) in values.iter().enumerate() {
                if index < MAX_ARRAY_ITEMS_PER_FIELD {
                    collect_field_paths(child, &path, depth + 1, fields, truncated);
                } else if value_has_unrepresented_shape(child, &path, depth + 1, fields) {
                    *truncated = true;
                    break;
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn collect_object_paths(
    map: &Map<String, Value>,
    prefix: &str,
    depth: usize,
    fields: &mut BTreeSet<String>,
    truncated: &mut bool,
) {
    for (index, (key, child)) in map.iter().enumerate() {
        let (segment, traverse_children) = match classify_field_segment(key) {
            FieldSegment::Structured(segment) => (segment, true),
            FieldSegment::Opaque(segment) => (segment, false),
            FieldSegment::Unknown => (UNKNOWN_FIELD_SEGMENT, false),
        };
        let path = format!("{prefix}.{segment}");

        if index >= MAX_OBJECT_FIELDS_PER_NODE {
            if !fields.contains(&path)
                || (traverse_children
                    && value_has_unrepresented_shape(child, &path, depth + 1, fields))
            {
                *truncated = true;
                break;
            }
            continue;
        }

        if !fields.contains(&path) && fields.len() >= MAX_FIELD_PATHS_PER_FRAME {
            *truncated = true;
            break;
        }
        fields.insert(path.clone());
        if traverse_children {
            collect_field_paths(child, &path, depth + 1, fields, truncated);
        }
    }
}

fn value_has_structural_children(value: &Value) -> bool {
    match value {
        Value::Object(map) => !map.is_empty(),
        // Arrays have an explicit `[]` shape path, including empty arrays.
        Value::Array(_) => true,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn value_has_unrepresented_shape(
    value: &Value,
    prefix: &str,
    depth: usize,
    fields: &BTreeSet<String>,
) -> bool {
    if depth >= MAX_FIELD_DEPTH {
        return value_has_structural_children(value);
    }

    match value {
        Value::Object(map) => map.iter().any(|(key, child)| {
            let (segment, traverse_children) = match classify_field_segment(key) {
                FieldSegment::Structured(segment) => (segment, true),
                FieldSegment::Opaque(segment) => (segment, false),
                FieldSegment::Unknown => (UNKNOWN_FIELD_SEGMENT, false),
            };
            let path = format!("{prefix}.{segment}");
            !fields.contains(&path)
                || (traverse_children
                    && value_has_unrepresented_shape(child, &path, depth + 1, fields))
        }),
        Value::Array(values) => {
            let path = format!("{prefix}[]");
            !fields.contains(&path)
                || values
                    .iter()
                    .any(|child| value_has_unrepresented_shape(child, &path, depth + 1, fields))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FieldSegment<'a> {
    Structured(&'a str),
    Opaque(&'a str),
    Unknown,
}

fn classify_field_segment(segment: &str) -> FieldSegment<'_> {
    if segment.is_empty() || segment.len() > MAX_FIELD_SEGMENT_BYTES {
        return FieldSegment::Unknown;
    }

    if is_opaque_field_segment(segment) {
        FieldSegment::Opaque(segment)
    } else if is_known_field_segment(segment) {
        FieldSegment::Structured(segment)
    } else {
        FieldSegment::Unknown
    }
}

/// Object-valued fields whose children are caller-controlled dictionaries or raw payloads.
///
/// Recording the container proves the capability was present while deliberately discarding its
/// keys. Keep this list independent of the structured allowlist: adding a field here is a one-way
/// reduction in diagnostic detail and must never expose additional wire data.
fn is_opaque_field_segment(segment: &str) -> bool {
    matches!(
        segment,
        "arguments"
            | "body"
            | "data"
            | "env"
            | "environment"
            | "headers"
            | "metadata"
            | "modelUsage"
            | "payload"
            | "properties"
            | "raw"
            | "variables"
    )
}

/// Reviewed ACP v1 and Grok extension schema field names that are safe to retain as shape data.
///
/// Syntactic validation is not sufficient here: UUIDs, tokens, paths, and other private values can
/// all be valid JSON object keys. Unknown keys therefore collapse to one placeholder and their
/// children are not traversed. Extend this allowlist only from a reviewed protocol/schema shape.
fn is_known_field_segment(segment: &str) -> bool {
    matches!(
        segment,
        "Ok" | "_meta"
            | "action"
            | "agentCapabilities"
            | "agentId"
            | "agentInstanceId"
            | "agentType"
            | "agentVersion"
            | "allow_access"
            | "audio"
            | "auth"
            | "authMethods"
            | "auto_mode"
            | "auto_permission_mode_enabled"
            | "availableCommands"
            | "availableModels"
            | "blockingEvents"
            | "cancelRewind"
            | "category"
            | "clientCapabilities"
            | "close"
            | "configOptions"
            | "content"
            | "createdAt"
            | "currentModeId"
            | "currentModelId"
            | "currentValue"
            | "currentWorkingDirectory"
            | "cwd"
            | "decisions"
            | "default"
            | "defaultAuthMethodId"
            | "description"
            | "elicitation"
            | "embeddedContext"
            | "entries"
            | "error"
            | "exitCode"
            | "exitStatus"
            | "form"
            | "fs"
            | "grokShell"
            | "hint"
            | "hostname"
            | "http"
            | "id"
            | "image"
            | "input"
            | "kind"
            | "label"
            | "line"
            | "list"
            | "loadSession"
            | "locations"
            | "mcpApps"
            | "mcpCapabilities"
            | "mcpServers"
            | "message"
            | "methodId"
            | "mimeType"
            | "modeId"
            | "model"
            | "modelId"
            | "modelState"
            | "model_id"
            | "models"
            | "name"
            | "optionId"
            | "options"
            | "output"
            | "path"
            | "permission_mode"
            | "plan"
            | "priority"
            | "prompt"
            | "promptCapabilities"
            | "protocolVersion"
            | "readTextFile"
            | "reasoningEffort"
            | "reasoningEfforts"
            | "reasoning_effort"
            | "removed"
            | "requestedSchema"
            | "required"
            | "resource"
            | "response"
            | "resume"
            | "sessionCapabilities"
            | "sessionConfig"
            | "sessionId"
            | "sessionRecap"
            | "sessionUpdate"
            | "sessions"
            | "signal"
            | "sse"
            | "status"
            | "stopReason"
            | "stopSignals"
            | "supportsReasoningEffort"
            | "terminal"
            | "terminalId"
            | "text"
            | "title"
            | "toolCallId"
            | "toolOverrides"
            | "totalContextTokens"
            | "type"
            | "update"
            | "updatedAt"
            | "upserted"
            | "uri"
            | "url"
            | "value"
            | "voiceMode"
            | "writeTextFile"
            | "x.ai/capabilities"
            | "x.ai/closeOutcome"
            | "x.ai/fs_notify"
            | "x.ai/hooks"
            | "x.ai/mcp/sdk"
            | "x.ai/pluginDirs"
            | "x.ai/sessionConfig"
            | "x_keyword_search"
            | "x_semantic_search"
            | "x_thread_fetch"
            | "x_user_search"
            | "yolo_mode"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn summarize_params_shape(params: Value) -> SanitizedWireFrame {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            &serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": params
            }))
            .unwrap(),
            LineDirection::Stdout,
        );
        recorder.summary.frames.remove(0)
    }

    fn nest_known_value(mut value: Value, levels: usize) -> Value {
        for _ in 0..levels {
            value = json!({"value": value});
        }
        value
    }

    fn reviewed_shape_keys() -> &'static [&'static str] {
        &[
            "Ok",
            "_meta",
            "action",
            "agentCapabilities",
            "agentId",
            "agentInstanceId",
            "agentType",
            "agentVersion",
            "allow_access",
            "audio",
            "auth",
            "authMethods",
            "auto_mode",
            "auto_permission_mode_enabled",
            "availableCommands",
            "availableModels",
            "blockingEvents",
            "cancelRewind",
            "category",
            "clientCapabilities",
            "close",
            "configOptions",
            "content",
            "createdAt",
            "currentModeId",
            "currentModelId",
            "currentValue",
            "currentWorkingDirectory",
            "cwd",
            "decisions",
            "default",
            "defaultAuthMethodId",
            "description",
            "elicitation",
            "embeddedContext",
            "entries",
            "error",
            "exitCode",
            "exitStatus",
            "form",
            "fs",
            "grokShell",
            "hint",
            "hostname",
            "http",
            "id",
            "image",
            "input",
            "kind",
            "label",
            "line",
            "list",
            "loadSession",
            "locations",
            "mcpApps",
            "mcpCapabilities",
            "mcpServers",
            "message",
            "methodId",
            "mimeType",
            "modeId",
            "model",
            "modelId",
            "modelState",
            "model_id",
            "models",
        ]
    }

    #[test]
    fn records_order_and_correlates_remapped_ids_without_payload_values() {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"private-id","method":"session/new","params":{"cwd":"C:\\Users\\Private","mcpServers":[]}}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"private-id","result":{"sessionId":"private-session"}}"#,
            LineDirection::Stdout,
        );

        let summary = recorder.summary();
        assert_eq!(summary.frames.len(), 2);
        assert_eq!(summary.frames[0].sequence, 1);
        assert_eq!(summary.frames[1].sequence, 2);
        assert_eq!(summary.frames[0].request_id, summary.frames[1].request_id);
        assert_eq!(
            summary.frames[0].request_id.as_deref(),
            Some("request-0001")
        );
        assert!(summary.frames[0].field_paths.contains("params.cwd"));
        assert!(summary.frames[1].field_paths.contains("result.sessionId"));
        assert!(recorder.requests.is_empty());
        let encoded = serde_json::to_string(&summary).unwrap();
        assert!(!encoded.contains("private-id"));
        assert!(!encoded.contains("Private"));
        assert!(!encoded.contains("private-session"));
    }

    #[test]
    fn official_out_of_band_cancellation_method_is_allowlisted_exactly() {
        assert!(is_known_method("$/cancel_request"));
        assert!(!is_known_method("$/cancelRequest"));

        let classification = classify_method(Some(&json!("$/cancel_request"))).unwrap();
        assert!(classification.valid);
        assert_eq!(classification.display, "$/cancel_request");
    }

    #[test]
    fn bounds_batches_arrays_frames_and_request_correlation_state() {
        let mut recorder = WireRecorder::default();
        let batch = Value::Array(
            (0..MAX_BATCH_ITEMS + 7)
                .map(|index| {
                    json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": { "sessionId": format!("private-{index}") }
                    })
                })
                .collect(),
        );
        recorder.record_line(
            &serde_json::to_string(&batch).unwrap(),
            LineDirection::Stdout,
        );
        let summary = recorder.summary();
        assert_eq!(summary.frames.len(), MAX_BATCH_ITEMS);
        assert_eq!(summary.dropped_frame_count, 7);

        let mut content = vec![json!({ "text": "private" }); MAX_ARRAY_ITEMS_PER_FIELD];
        content.push(json!({ "cwd": "C:/Users/ExampleUser/private" }));
        let line = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "content": content }
        });
        let mut array_recorder = WireRecorder::default();
        array_recorder.record_line(
            &serde_json::to_string(&line).unwrap(),
            LineDirection::Stdout,
        );
        let fields = &array_recorder.summary().frames[0].field_paths;
        assert!(fields.contains("params.content[].text"));
        assert!(!fields.contains("params.content[].cwd"));

        let mut frame_recorder = WireRecorder::default();
        for id in 0..MAX_RECORDED_FRAMES + 32 {
            frame_recorder.record_line(
                &format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{}}}}"#),
                LineDirection::Stdin,
            );
        }
        assert_eq!(frame_recorder.summary.frames.len(), MAX_RECORDED_FRAMES);
        assert_eq!(frame_recorder.requests.len(), MAX_RECORDED_FRAMES);
        assert_eq!(frame_recorder.summary.dropped_frame_count, 32);
    }

    #[test]
    fn empty_and_nested_batches_are_unknown_without_flattening() {
        let mut recorder = WireRecorder::default();
        recorder.record_line("[]", LineDirection::Stdin);
        recorder.record_line(
            r#"[
                [{"jsonrpc":"2.0","id":"nested-private","method":"initialize"}],
                {"jsonrpc":"2.0","id":"direct-private","method":"initialize"},
                []
            ]"#,
            LineDirection::Stdin,
        );

        let summary = recorder.summary();
        assert_eq!(summary.frames.len(), 4);
        assert_eq!(summary.frames[0].kind, WireFrameKind::Unknown);
        assert_eq!(summary.frames[1].kind, WireFrameKind::Unknown);
        assert_eq!(summary.frames[2].kind, WireFrameKind::Request);
        assert_eq!(
            summary.frames[2].request_id.as_deref(),
            Some("request-0001")
        );
        assert_eq!(summary.frames[3].kind, WireFrameKind::Unknown);
        assert_eq!(recorder.requests.len(), 1);

        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"direct-private","result":{}}"#,
            LineDirection::Stdout,
        );
        assert!(recorder.requests.is_empty());
        let encoded = serde_json::to_string(&recorder.summary()).unwrap();
        assert!(!encoded.contains("nested-private"));
        assert!(!encoded.contains("direct-private"));
    }

    #[test]
    fn field_path_depth_cap_reports_only_hidden_structure() {
        let scalar_at_boundary = nest_known_value(json!(1), MAX_FIELD_DEPTH);
        let empty_at_boundary = nest_known_value(json!({}), MAX_FIELD_DEPTH);
        let empty_array_at_boundary = nest_known_value(json!([]), MAX_FIELD_DEPTH);
        let structure_at_boundary = nest_known_value(json!({"value": 1}), MAX_FIELD_DEPTH);

        let scalar_frame = summarize_params_shape(scalar_at_boundary);
        let empty_frame = summarize_params_shape(empty_at_boundary);
        let empty_array_frame = summarize_params_shape(empty_array_at_boundary);
        let structure_frame = summarize_params_shape(structure_at_boundary);
        assert!(!scalar_frame.field_paths_truncated);
        assert!(!empty_frame.field_paths_truncated);
        assert!(empty_array_frame.field_paths_truncated);
        assert!(structure_frame.field_paths_truncated);
        assert!(
            serde_json::to_value(&structure_frame).unwrap()["fieldPathsTruncated"]
                .as_bool()
                .unwrap()
        );
        assert!(
            serde_json::to_value(&scalar_frame)
                .unwrap()
                .get("fieldPathsTruncated")
                .is_none()
        );
    }

    #[test]
    fn object_field_cap_reports_an_omitted_distinct_shape() {
        let mut object = Map::new();
        for index in 0..MAX_OBJECT_FIELDS_PER_NODE {
            object.insert(format!("a{index:03}"), json!(index));
        }
        object.insert("value".to_owned(), json!(1));

        let frame = summarize_params_shape(Value::Object(object));
        assert!(frame.field_paths.contains("params.<unknown-key>"));
        assert!(!frame.field_paths.contains("params.value"));
        assert!(frame.field_paths_truncated);
    }

    #[test]
    fn array_item_cap_reports_hidden_shape_but_not_extra_scalars() {
        let mut with_hidden_shape = vec![json!(0); MAX_ARRAY_ITEMS_PER_FIELD];
        with_hidden_shape.push(json!({"value": 1}));
        let hidden_frame = summarize_params_shape(json!({"content": with_hidden_shape}));
        assert!(hidden_frame.field_paths_truncated);
        assert!(!hidden_frame.field_paths.contains("params.content[].value"));

        let mut with_hidden_empty_array = vec![json!(0); MAX_ARRAY_ITEMS_PER_FIELD];
        with_hidden_empty_array.push(json!([]));
        let hidden_empty_array_frame =
            summarize_params_shape(json!({"content": with_hidden_empty_array}));
        assert!(hidden_empty_array_frame.field_paths_truncated);
        assert!(
            !hidden_empty_array_frame
                .field_paths
                .contains("params.content[][]")
        );

        let scalars_only = vec![json!(0); MAX_ARRAY_ITEMS_PER_FIELD + 1];
        let scalar_frame = summarize_params_shape(json!({"content": scalars_only}));
        assert!(!scalar_frame.field_paths_truncated);
    }

    #[test]
    fn global_field_path_cap_reports_only_an_actual_omitted_path() {
        let keys = reviewed_shape_keys();
        let complete = Value::Object(
            keys.iter()
                .take(MAX_FIELD_PATHS_PER_FRAME - 1)
                .map(|key| ((*key).to_owned(), json!(1)))
                .collect(),
        );
        let truncated = Value::Object(
            keys.iter()
                .take(MAX_FIELD_PATHS_PER_FRAME)
                .map(|key| ((*key).to_owned(), json!(1)))
                .collect(),
        );

        let complete_frame = summarize_params_shape(complete);
        assert_eq!(complete_frame.field_paths.len(), MAX_FIELD_PATHS_PER_FRAME);
        assert!(!complete_frame.field_paths_truncated);

        let truncated_frame = summarize_params_shape(truncated);
        assert_eq!(truncated_frame.field_paths.len(), MAX_FIELD_PATHS_PER_FRAME);
        assert!(truncated_frame.field_paths_truncated);
    }

    #[test]
    fn global_field_path_cap_reports_an_omitted_empty_array_container() {
        let keys = reviewed_shape_keys();
        let mut params: Map<String, Value> = keys
            .iter()
            .take(MAX_FIELD_PATHS_PER_FRAME - 2)
            .map(|key| ((*key).to_owned(), json!(1)))
            .collect();
        params.insert("models".to_owned(), json!([]));

        let frame = summarize_params_shape(Value::Object(params));

        assert_eq!(frame.field_paths.len(), MAX_FIELD_PATHS_PER_FRAME);
        assert!(frame.field_paths.contains("params.models"));
        assert!(!frame.field_paths.contains("params.models[]"));
        assert!(frame.field_paths_truncated);
    }

    #[test]
    fn global_field_path_cap_also_bounds_additional_envelope_roots() {
        let params: Map<String, Value> = reviewed_shape_keys()
            .iter()
            .take(MAX_FIELD_PATHS_PER_FRAME - 1)
            .map(|key| ((*key).to_owned(), json!(1)))
            .collect();
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            &serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "params": params,
                "result": {},
                "error": {"code": -32000, "message": "private"}
            }))
            .unwrap(),
            LineDirection::Stdout,
        );

        let frame = &recorder.summary.frames[0];
        assert_eq!(frame.kind, WireFrameKind::Unknown);
        assert_eq!(frame.field_paths.len(), MAX_FIELD_PATHS_PER_FRAME);
        assert!(!frame.field_paths.contains("result"));
        assert!(!frame.field_paths.contains("error"));
        assert!(frame.field_paths_truncated);
    }

    #[test]
    fn invalid_request_ids_make_frames_unknown_without_touching_correlation() {
        let oversized_id = "secret".repeat(MAX_REQUEST_ID_BYTES);
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            &serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": oversized_id,
                "method": "initialize",
                "params": {}
            }))
            .unwrap(),
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":{"private":"value"},"result":{}}"#,
            LineDirection::Stdout,
        );

        let summary = recorder.summary();
        assert_eq!(summary.untracked_request_id_count, 0);
        assert!(
            summary
                .frames
                .iter()
                .all(|frame| frame.kind == WireFrameKind::Unknown && frame.request_id.is_none())
        );
        assert!(recorder.requests.is_empty());
        assert!(
            !serde_json::to_string(&summary)
                .unwrap()
                .contains("secretsecret")
        );
    }

    #[test]
    fn keeps_client_and_agent_request_id_namespaces_separate() {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"session/request_permission","params":{}}"#,
            LineDirection::Stdout,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            LineDirection::Stdin,
        );

        let summary = recorder.summary();
        assert_eq!(
            summary.frames[0].request_id.as_deref(),
            Some("request-0001")
        );
        assert_eq!(
            summary.frames[1].request_id.as_deref(),
            Some("request-0002")
        );
        assert_eq!(
            summary.frames[2].request_id.as_deref(),
            Some("request-0002")
        );
    }

    #[test]
    fn retires_correlations_and_allows_sequential_id_reuse() {
        let mut recorder = WireRecorder::default();
        for method in ["initialize", "session/new"] {
            recorder.record_line(
                &format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{}}}}"#),
                LineDirection::Stdin,
            );
            recorder.record_line(
                r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
                LineDirection::Stdout,
            );
        }

        let summary = recorder.summary();
        assert_eq!(
            summary.frames[0].request_id.as_deref(),
            Some("request-0001")
        );
        assert_eq!(summary.frames[0].request_id, summary.frames[1].request_id);
        assert_eq!(
            summary.frames[2].request_id.as_deref(),
            Some("request-0002")
        );
        assert_eq!(summary.frames[2].request_id, summary.frames[3].request_id);
        assert!(recorder.requests.is_empty());
    }

    #[test]
    fn duplicate_outstanding_and_orphan_response_ids_fail_closed() {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"same","method":"initialize","params":{}}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"same","method":"session/new","params":{}}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"same","result":{}}"#,
            LineDirection::Stdout,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"orphan","result":{}}"#,
            LineDirection::Stdout,
        );

        let summary = recorder.summary();
        assert_eq!(
            summary.frames[0].request_id.as_deref(),
            Some("request-0001")
        );
        assert_eq!(
            summary.frames[1].request_id.as_deref(),
            Some(DUPLICATE_REQUEST_ID)
        );
        assert_eq!(summary.frames[0].request_id, summary.frames[2].request_id);
        assert_eq!(
            summary.frames[3].request_id.as_deref(),
            Some(UNTRACKED_REQUEST_ID)
        );
        assert_eq!(summary.duplicate_request_id_count, 1);
        assert_eq!(summary.untracked_request_id_count, 1);
        assert!(recorder.requests.is_empty());
    }

    #[test]
    fn invalid_method_and_unknown_field_names_cannot_smuggle_payload_text() {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            r#"{"jsonrpc":"2.0","method":"future method secret","params":{"secret value":"do-not-store"}}"#,
            LineDirection::Stdout,
        );

        let summary = recorder.summary();
        assert_eq!(summary.frames[0].kind, WireFrameKind::Unknown);
        assert_eq!(summary.frames[0].method.as_deref(), Some(INVALID_METHOD));
        let encoded = serde_json::to_string(&summary).unwrap();
        assert!(encoded.contains(INVALID_METHOD));
        assert!(encoded.contains(UNKNOWN_FIELD_SEGMENT));
        assert!(!encoded.contains("future method secret"));
        assert!(!encoded.contains("secret value"));
        assert!(!encoded.contains("do-not-store"));
    }

    #[test]
    fn valid_but_unreviewed_methods_collapse_and_still_correlate() {
        let private_method = "x.ai/C:/Users/Private/Bearer_private_token";
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            &serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": "private-request-id",
                "method": private_method,
                "params": {}
            }))
            .unwrap(),
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"private-request-id","result":{}}"#,
            LineDirection::Stdout,
        );

        let summary = recorder.summary();
        assert_eq!(summary.frames[0].kind, WireFrameKind::Request);
        assert_eq!(summary.frames[0].method.as_deref(), Some(UNKNOWN_METHOD));
        assert_eq!(summary.frames[0].request_id, summary.frames[1].request_id);
        assert!(summary.client_methods.contains(UNKNOWN_METHOD));
        assert!(recorder.requests.is_empty());

        let encoded = serde_json::to_string(&summary).unwrap();
        assert!(encoded.contains(UNKNOWN_METHOD));
        for forbidden in [
            private_method,
            "C:/Users/Private",
            "Bearer_private_token",
            "private-request-id",
        ] {
            assert!(!encoded.contains(forbidden), "summary contains {forbidden}");
        }
    }

    #[test]
    fn malformed_and_ambiguous_envelopes_never_mutate_correlation() {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"stable","method":"initialize","params":{}}"#,
            LineDirection::Stdin,
        );

        let malformed = [
            json!({"id": "stable", "method": "initialize", "params": {}}),
            json!({"jsonrpc": "1.0", "id": "stable", "method": "initialize"}),
            json!({"jsonrpc": "2.0", "id": "stable", "method": 7}),
            json!({"jsonrpc": "2.0", "id": "stable", "method": "bad method"}),
            json!({"jsonrpc": "2.0", "id": null, "method": "initialize"}),
            json!({
                "jsonrpc": "2.0",
                "id": "stable",
                "method": "initialize",
                "result": {}
            }),
            json!({"jsonrpc": "2.0", "id": "stable", "result": {}, "error": {}}),
            json!({
                "jsonrpc": "2.0",
                "id": "stable",
                "result": {},
                "params": {"secret": "private-result-param"}
            }),
            json!({
                "jsonrpc": "2.0",
                "id": "stable",
                "error": {"code": -32000, "message": "failed"},
                "params": {"secret": "private-error-param"}
            }),
            json!({"jsonrpc": "2.0", "id": "stable"}),
            json!({"jsonrpc": "2.0", "id": {"private": "id"}, "result": {}}),
        ];
        for frame in malformed {
            recorder.record_line(
                &serde_json::to_string(&frame).unwrap(),
                LineDirection::Stdout,
            );
        }

        assert_eq!(recorder.requests.len(), 1);
        assert_eq!(recorder.summary.untracked_request_id_count, 0);
        assert_eq!(recorder.summary.duplicate_request_id_count, 0);
        assert!(
            recorder.summary.frames[1..]
                .iter()
                .all(|frame| frame.kind == WireFrameKind::Unknown && frame.request_id.is_none())
        );

        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"stable","result":{}}"#,
            LineDirection::Stdout,
        );
        assert!(recorder.requests.is_empty());
        assert_eq!(
            recorder
                .summary
                .frames
                .last()
                .unwrap()
                .request_id
                .as_deref(),
            Some("request-0001")
        );
    }

    #[test]
    fn invalid_params_and_error_objects_never_create_or_retire_correlation() {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"stable","method":"initialize","params":{}}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"stable","method":"initialize","params":"private-string"}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":null}"#,
            LineDirection::Stdout,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"stable","error":null}"#,
            LineDirection::Stdout,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"stable","error":{"code":-32000,"message":7}}"#,
            LineDirection::Stdout,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"stable","error":{"code":1.5,"message":"bad"}}"#,
            LineDirection::Stdout,
        );

        assert_eq!(recorder.requests.len(), 1);
        assert_eq!(recorder.summary.duplicate_request_id_count, 0);
        assert_eq!(recorder.summary.untracked_request_id_count, 0);
        assert!(
            recorder.summary.frames[1..]
                .iter()
                .all(|frame| frame.kind == WireFrameKind::Unknown && frame.request_id.is_none())
        );

        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"stable","error":{"code":-32000,"message":"failed","data":null}}"#,
            LineDirection::Stdout,
        );
        let final_frame = recorder.summary.frames.last().unwrap();
        assert_eq!(final_frame.kind, WireFrameKind::ErrorResponse);
        assert_eq!(final_frame.request_id.as_deref(), Some("request-0001"));
        assert!(recorder.requests.is_empty());

        let encoded = serde_json::to_string(&recorder.summary()).unwrap();
        for forbidden in ["private-string", "failed", "stable"] {
            assert!(!encoded.contains(forbidden), "summary contains {forbidden}");
        }
    }

    #[test]
    fn string_and_number_request_ids_have_exact_private_identities() {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"1","method":"session/new","params":{}}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":"1","result":{}}"#,
            LineDirection::Stdout,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            LineDirection::Stdout,
        );

        let summary = recorder.summary();
        assert_eq!(
            summary.frames[0].request_id.as_deref(),
            Some("request-0001")
        );
        assert_eq!(
            summary.frames[1].request_id.as_deref(),
            Some("request-0002")
        );
        assert_eq!(summary.frames[1].request_id, summary.frames[2].request_id);
        assert_eq!(summary.frames[0].request_id, summary.frames[3].request_id);
        assert!(recorder.requests.is_empty());

        let encoded = serde_json::to_string(&summary).unwrap();
        assert!(!encoded.contains("\"id\":\"1\""));
        assert!(!encoded.contains("\"id\":1"));
    }

    #[test]
    fn numeric_request_ids_require_exact_integer_representation() {
        let mut recorder = WireRecorder::default();
        for invalid in [
            r#"{"jsonrpc":"2.0","id":1.5,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1e3,"method":"initialize","params":{}}"#,
        ] {
            recorder.record_line(invalid, LineDirection::Stdin);
        }
        assert!(
            recorder
                .summary
                .frames
                .iter()
                .all(|frame| frame.kind == WireFrameKind::Unknown && frame.request_id.is_none())
        );
        assert!(recorder.requests.is_empty());

        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":-9223372036854775808,"method":"initialize","params":{}}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":18446744073709551615,"method":"session/new","params":{}}"#,
            LineDirection::Stdin,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":18446744073709551615,"result":{}}"#,
            LineDirection::Stdout,
        );
        recorder.record_line(
            r#"{"jsonrpc":"2.0","id":-9223372036854775808,"result":{}}"#,
            LineDirection::Stdout,
        );

        assert_eq!(
            recorder.summary.frames[2].request_id.as_deref(),
            Some("request-0001")
        );
        assert_eq!(
            recorder.summary.frames[3].request_id.as_deref(),
            Some("request-0002")
        );
        assert_eq!(
            recorder.summary.frames[3].request_id,
            recorder.summary.frames[4].request_id
        );
        assert_eq!(
            recorder.summary.frames[2].request_id,
            recorder.summary.frames[5].request_id
        );
        assert!(recorder.requests.is_empty());
    }

    #[test]
    fn valid_json_key_syntax_cannot_smuggle_secrets_paths_or_identifiers() {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            r#"{
                "jsonrpc":"2.0",
                "method":"session/update",
                "params":{
                    "plainAlphanumericSecret123":{"sessionId":"nested-private-session"},
                    "550e8400-e29b-41d4-a716-446655440000":{"modelId":"nested-private-model"},
                    "C:/Users/ExampleUser/private":{"cwd":"nested-private-path"},
                    "modelUsage":{"grok-4.6-build":{"inputTokens":42}},
                    "env":{"XAI_API_KEY":"private-key"},
                    "headers":{"Authorization":"Bearer private-token"},
                    "payload":{"anotherSecretKey":"private-payload"}
                }
            }"#,
            LineDirection::Stdout,
        );

        let summary = recorder.summary();
        let fields = &summary.frames[0].field_paths;
        assert!(fields.contains("params"));
        assert!(fields.contains("params.<unknown-key>"));
        for opaque_container in ["env", "headers", "modelUsage", "payload"] {
            let path = format!("params.{opaque_container}");
            assert!(fields.contains(&path));
            assert!(
                !fields
                    .iter()
                    .any(|field| field.starts_with(&format!("{path}.")))
            );
        }

        let encoded = serde_json::to_string(&summary).unwrap();
        for forbidden in [
            "plainAlphanumericSecret123",
            "550e8400-e29b-41d4-a716-446655440000",
            "C:/Users/ExampleUser/private",
            "grok-4.6-build",
            "inputTokens",
            "XAI_API_KEY",
            "Authorization",
            "anotherSecretKey",
            "nested-private-session",
            "private-token",
        ] {
            assert!(!encoded.contains(forbidden), "summary contains {forbidden}");
        }
    }

    #[test]
    fn reviewed_initialize_and_control_schema_fields_remain_visible() {
        let mut recorder = WireRecorder::default();
        recorder.record_line(
            r#"{
                "jsonrpc":"2.0",
                "id":1,
                "result":{
                    "_meta":{
                        "modelState":{
                            "currentModelId":"private-model-value",
                            "availableModels":[{
                                "modelId":"private-model-value",
                                "_meta":{
                                    "reasoningEffort":"private-effort-value",
                                    "reasoningEfforts":[{"id":"private-id-value","default":true}]
                                }
                            }]
                        },
                        "x.ai/mcp/sdk":true
                    },
                    "agentCapabilities":{"sessionCapabilities":{"list":true,"resume":true,"close":true}}
                }
            }"#,
            LineDirection::Stdout,
        );
        recorder.record_line(
            r#"{
                "jsonrpc":"2.0",
                "id":2,
                "method":"session/set_model",
                "params":{
                    "sessionId":"private-session-value",
                    "modelId":"private-model-value",
                    "_meta":{"reasoningEffort":"private-effort-value"}
                }
            }"#,
            LineDirection::Stdin,
        );

        let summary = recorder.summary();
        let initialize_fields = &summary.frames[0].field_paths;
        for expected in [
            "result._meta.modelState.availableModels[].modelId",
            "result._meta.modelState.availableModels[]._meta.reasoningEffort",
            "result._meta.modelState.availableModels[]._meta.reasoningEfforts[].id",
            "result._meta.x.ai/mcp/sdk",
            "result.agentCapabilities.sessionCapabilities.resume",
        ] {
            assert!(
                initialize_fields.contains(expected),
                "missing reviewed initialize shape {expected}"
            );
        }
        let control_fields = &summary.frames[1].field_paths;
        for expected in [
            "params.sessionId",
            "params.modelId",
            "params._meta.reasoningEffort",
        ] {
            assert!(
                control_fields.contains(expected),
                "missing reviewed control shape {expected}"
            );
        }

        let encoded = serde_json::to_string(&summary).unwrap();
        for forbidden in [
            "private-model-value",
            "private-effort-value",
            "private-id-value",
            "private-session-value",
        ] {
            assert!(!encoded.contains(forbidden), "summary contains {forbidden}");
        }
    }

    #[test]
    fn checked_in_installed_runtime_shape_is_sanitized_and_correlated() {
        let fixture =
            include_str!("../../../fixtures/acp/installed-grok-1.0.5-initialize-shape.json");
        let value: Value = serde_json::from_str(fixture).unwrap();
        let frames = value["frames"].as_array().unwrap();

        assert_eq!(frames.len(), 2);
        assert_eq!(value["schemaVersion"], 2);
        assert_eq!(frames[0]["requestId"], "request-0001");
        assert_eq!(frames[1]["requestId"], "request-0001");
        assert_eq!(frames[0]["method"], "initialize");
        assert_eq!(frames[1]["fieldPaths"].as_array().unwrap().len(), 64);
        assert_eq!(frames[1]["fieldPathsTruncated"], true);
        assert!(fixture.contains("result._meta.modelState.availableModels[].modelId"));

        for forbidden in [
            "C:\\\\Users",
            "C:/Users/",
            "auth.json",
            "cached_token",
            "private-session",
            "api_key=",
        ] {
            assert!(!fixture.contains(forbidden), "fixture contains {forbidden}");
        }
    }

    #[test]
    fn checked_in_control_shape_is_zero_turn_sanitized_and_correlated() {
        let fixture =
            include_str!("../../../fixtures/acp/installed-grok-1.0.5-controls-shape.json");
        let value: Value = serde_json::from_str(fixture).unwrap();
        let frames = value["frames"].as_array().unwrap();

        assert_eq!(frames.len(), 15);
        assert_eq!(value["schemaVersion"], 2);
        assert!(frames.iter().all(|frame| {
            frame["fieldPaths"]
                .as_array()
                .is_none_or(|paths| paths.len() <= MAX_FIELD_PATHS_PER_FRAME)
        }));
        assert_eq!(frames[0]["requestId"], "request-0001");
        assert_eq!(frames[2]["requestId"], "request-0001");
        assert_eq!(frames[3]["requestId"], "request-0002");
        assert_eq!(frames[4]["requestId"], "request-0002");
        assert_eq!(frames[12]["requestId"], "request-0005");
        assert_eq!(frames[14]["requestId"], "request-0005");
        assert_eq!(value["observations"]["promptSent"], false);
        assert_eq!(
            value["observations"]["globalPermissionNotificationSent"],
            false
        );
        assert!(fixture.contains("result._meta.model.Ok"));
        assert!(!fixture.contains("session/prompt"));
        assert!(!fixture.contains("_x.ai/yolo_mode_changed"));

        for forbidden in [
            "C:\\\\Users",
            "C:/Users/",
            "auth.json",
            "cached_token",
            "private-session",
            "api_key=",
        ] {
            assert!(!fixture.contains(forbidden), "fixture contains {forbidden}");
        }
    }

    #[test]
    fn checked_in_lifecycle_shape_preserves_replay_order_without_payload_values() {
        let fixture =
            include_str!("../../../fixtures/acp/installed-grok-1.0.5-lifecycle-shape.json");
        let value: Value = serde_json::from_str(fixture).unwrap();
        let wire = &value["wire"];
        let frames = wire["frames"].as_array().unwrap();
        let client_methods = wire["clientMethods"].as_array().unwrap();

        assert_eq!(frames.len(), 121);
        assert_eq!(value["schemaVersion"], 2);
        assert!(wire.get("methodShapes").is_none());
        assert_eq!(wire["droppedFrameCount"], 0);
        assert_eq!(wire["malformedStdoutLines"], 0);
        assert!(frames.iter().all(|frame| {
            frame["fieldPaths"]
                .as_array()
                .is_none_or(|paths| paths.len() <= MAX_FIELD_PATHS_PER_FRAME)
        }));
        assert!(
            frames
                .iter()
                .any(|frame| frame["fieldPathsTruncated"] == true)
        );
        for required in [
            "initialize",
            "authenticate",
            "session/new",
            "session/prompt",
            "session/list",
            "session/load",
            "session/resume",
            "session/close",
        ] {
            assert!(
                client_methods.iter().any(|method| method == required),
                "lifecycle fixture omits {required}"
            );
        }

        let load_request = frames
            .iter()
            .find(|frame| frame["method"] == "session/load")
            .unwrap();
        let load_id = &load_request["requestId"];
        let load_request_sequence = load_request["sequence"].as_u64().unwrap();
        let load_response_sequence = frames
            .iter()
            .find(|frame| frame["kind"] == "response" && &frame["requestId"] == load_id)
            .and_then(|frame| frame["sequence"].as_u64())
            .unwrap();
        assert!(frames.iter().any(|frame| {
            let sequence = frame["sequence"].as_u64().unwrap();
            sequence > load_request_sequence
                && sequence < load_response_sequence
                && matches!(
                    frame["method"].as_str(),
                    Some("session/update" | "_x.ai/session/update")
                )
        }));

        let resume_request = frames
            .iter()
            .find(|frame| frame["method"] == "session/resume")
            .unwrap();
        let resume_id = &resume_request["requestId"];
        let resume_request_sequence = resume_request["sequence"].as_u64().unwrap();
        let resume_response_sequence = frames
            .iter()
            .find(|frame| frame["kind"] == "response" && &frame["requestId"] == resume_id)
            .and_then(|frame| frame["sequence"].as_u64())
            .unwrap();
        assert!(!frames.iter().any(|frame| {
            let sequence = frame["sequence"].as_u64().unwrap();
            sequence > resume_request_sequence
                && sequence < resume_response_sequence
                && frame["fieldPaths"].as_array().is_some_and(|paths| {
                    paths
                        .iter()
                        .any(|path| path == "params.update.content.text")
                })
        }));

        for forbidden in [
            "C:\\\\Users",
            "C:/Users/",
            "auth.json",
            "cached_token",
            "GROK_BUILD_GUI_COMPAT_OK",
            "Reply with exactly",
            "api_key=",
            "Bearer ",
        ] {
            assert!(!fixture.contains(forbidden), "fixture contains {forbidden}");
        }
    }
}
