//! Privacy-minimized normalization for Grok's session lifecycle responses.
//!
//! Grok currently places model state and session controls in response fields
//! that are outside standard ACP v1. Raw JSON must stop here: application
//! callers receive only typed, bounded values and safe object-key summaries.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    GrokExtensionOutcome, ModelCatalog,
    grok_extension::{
        MAX_DESCRIPTION_BYTES, MAX_DISPLAY_TEXT_BYTES, MAX_IDENTIFIER_BYTES, MAX_MODELS,
        MAX_PROTOCOL_TOKEN_LEN as MAX_PROTOCOL_TOKEN_BYTES, MAX_REASONING_EFFORTS,
    },
    normalize_grok_extension, redact_diagnostic,
};

const MAX_CONFIG_OPTIONS: usize = 64;
const MAX_RESPONSE_ID_BYTES: usize = 128;

const UNKNOWN_RESPONSE_KEY: &str = "<unknown-key>";
const SAFE_RESPONSE_KEY_ALLOWLIST: &[&str] = &[
    "_meta",
    "sessionId",
    "models",
    "modelState",
    "currentModelId",
    "availableModels",
    "configOptions",
    "modes",
    "x.ai/sessionConfig",
    "_x.ai/sessionConfig",
    "sessionConfig",
    "x.ai/closeOutcome",
    "codebaseIndexed",
    "feedbackEnabled",
    "isGitRepo",
    "showNonGitWarning",
    "x.ai/schedulerBackgroundLoops",
];
// By construction, a summary contains each allowlisted key at most once plus
// one collapsed marker for every unknown key. Truncation therefore cannot hide
// a reviewed key.
const MAX_SUMMARY_KEYS: usize = SAFE_RESPONSE_KEY_ALLOWLIST.len() + 1;

/// Normalize a raw JSON-RPC result envelope or a bare session result.
///
/// This deliberately does not retain the session id, workspace metadata,
/// paths, transcript detail, arbitrary extension values, or parse errors.
#[must_use]
pub fn normalize_grok_session_response(response: Value) -> GrokSessionResponse {
    let Some(body) = response_body(&response) else {
        return GrokSessionResponse::default();
    };
    let metadata = body.get("_meta");

    GrokSessionResponse {
        models: normalize_body_model_state(body),
        config_options: metadata
            .and_then(find_session_config)
            .and_then(normalize_config_options),
        response_keys: SafeResponseKeys::from_value(body),
        metadata_keys: metadata
            .map_or_else(SafeResponseKeys::default, SafeResponseKeys::from_value),
    }
}

/// Application-facing projection of a Grok new/load/resume response.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct GrokSessionResponse {
    /// Session-local model state, when the response supplied a complete,
    /// internally consistent catalog.
    pub models: Option<ModelCatalog>,
    /// Grok's legacy model/reasoning selectors from
    /// `_meta["x.ai/sessionConfig"].options`.
    /// A complete, usable selector snapshot. `None` means the field was absent
    /// or the entire snapshot was rejected as malformed or oversized.
    pub config_options: Option<Vec<GrokSessionConfigOption>>,
    /// Bounded safe names from the response object. Values are never retained.
    pub response_keys: SafeResponseKeys,
    /// Bounded safe names from `_meta`. Path and credential-bearing keys are
    /// excluded as well as their values.
    pub metadata_keys: SafeResponseKeys,
}

/// One legacy Grok session selector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GrokSessionConfigOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub selected: Option<bool>,
}

/// Bounded object-key names retained after opaque response data is discarded.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SafeResponseKeys(Vec<String>);

impl SafeResponseKeys {
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    fn from_value(value: &Value) -> Self {
        let Some(object) = value.as_object() else {
            return Self::default();
        };

        let mut has_unknown = false;
        let mut keys: Vec<String> = object
            .keys()
            .filter_map(|key| match safe_summary_key(key) {
                Some(key) => Some(key.to_owned()),
                None => {
                    has_unknown = true;
                    None
                }
            })
            .collect();
        if has_unknown {
            keys.push(UNKNOWN_RESPONSE_KEY.to_owned());
        }
        keys.sort_unstable();
        keys.dedup();
        debug_assert!(keys.len() <= MAX_SUMMARY_KEYS);
        keys.truncate(MAX_SUMMARY_KEYS);
        Self(keys)
    }
}

fn response_body(response: &Value) -> Option<&Value> {
    let object = response.as_object()?;
    let looks_like_json_rpc_envelope = object.contains_key("jsonrpc")
        || object.contains_key("id")
        || object.contains_key("method")
        || object.contains_key("params")
        || object.contains_key("result")
        || object.contains_key("error");

    if looks_like_json_rpc_envelope {
        let valid_version = object.get("jsonrpc") == Some(&Value::String("2.0".to_owned()));
        let is_response = valid_response_id(object.get("id"))
            && !object.contains_key("method")
            && !object.contains_key("params");
        let has_result_only = object.contains_key("result") && !object.contains_key("error");
        if !valid_version || !is_response || !has_result_only {
            return None;
        }

        return object.get("result").filter(|result| result.is_object());
    }

    Some(response)
}

fn valid_response_id(id: Option<&Value>) -> bool {
    match id {
        Some(Value::String(value)) => value.len() <= MAX_RESPONSE_ID_BYTES,
        Some(Value::Number(value)) if value.is_i64() || value.is_u64() => {
            value.to_string().len() <= MAX_RESPONSE_ID_BYTES
        }
        Some(
            Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) | Value::Number(_),
        )
        | None => false,
    }
}

fn normalize_body_model_state(body: &Value) -> Option<ModelCatalog> {
    let nested = select_consistent_alias([
        body.get("models"),
        body.get("modelState"),
        body.pointer("/_meta/modelState"),
        body.pointer("/_meta/models"),
    ])
    .ok()?;

    let has_flattened_alias =
        body.get("currentModelId").is_some() || body.get("availableModels").is_some();
    let flattened = if has_flattened_alias {
        Some(serde_json::json!({
            "currentModelId": body.get("currentModelId")?.clone(),
            "availableModels": body.get("availableModels")?.clone(),
        }))
    } else {
        None
    };

    match (nested, flattened.as_ref()) {
        (None, None) => None,
        (Some(raw), None) => normalize_model_state(raw),
        (None, Some(raw)) => normalize_model_state(raw),
        (Some(nested_raw), Some(flattened_raw)) => {
            let nested_catalog = normalize_model_state(nested_raw)?;
            let flattened_catalog = normalize_model_state(flattened_raw)?;
            (nested_catalog == flattened_catalog).then_some(nested_catalog)
        }
    }
}

/// Select a snapshot from compatibility aliases without making key order a
/// hidden precedence rule. Null has the same meaning as an absent optional
/// alias; multiple non-null aliases are accepted only when their values are
/// exactly equal.
fn select_consistent_alias<'a, I>(candidates: I) -> Result<Option<&'a Value>, ()>
where
    I: IntoIterator<Item = Option<&'a Value>>,
{
    let mut selected = None;
    for candidate in candidates
        .into_iter()
        .flatten()
        .filter(|value| !value.is_null())
    {
        match selected {
            None => selected = Some(candidate),
            Some(previous) if previous == candidate => {}
            Some(_) => return Err(()),
        }
    }
    Ok(selected)
}

fn normalize_model_state(raw: &Value) -> Option<ModelCatalog> {
    let object = raw.as_object()?;
    let current_model_id = bounded_identity(object.get("currentModelId")?, MAX_IDENTIFIER_BYTES)?;

    let available_models: &[Value] = match object.get("availableModels") {
        Some(Value::Array(models)) => models,
        Some(_) => return None,
        // ModelCatalog represents a complete snapshot. A current model without
        // its advertised catalog is not silently reinterpreted as an empty one.
        None => return None,
    };
    if available_models.len() > MAX_MODELS {
        return None;
    }

    let mut model_ids = BTreeSet::new();
    let mut safe_models = Vec::with_capacity(available_models.len());
    for raw_model in available_models {
        let safe_model = safe_model_value(raw_model)?;
        let model_id = safe_model.get("modelId")?.as_str()?;
        if !model_ids.insert(model_id.to_owned()) {
            return None;
        }
        safe_models.push(safe_model);
    }

    if !model_ids.contains(&current_model_id) {
        return None;
    }

    let safe_payload = serde_json::json!({
        "currentModelId": current_model_id,
        "availableModels": safe_models,
    });

    match normalize_grok_extension("x.ai/models/update", safe_payload).ok()? {
        GrokExtensionOutcome::ModelsUpdated(catalog) => Some(catalog),
        _ => None,
    }
}

fn safe_model_value(raw: &Value) -> Option<Value> {
    let model = raw.as_object()?;
    let mut safe = Map::new();
    safe.insert(
        "modelId".to_owned(),
        Value::String(bounded_identity(
            model.get("modelId")?,
            MAX_IDENTIFIER_BYTES,
        )?),
    );
    safe.insert(
        "name".to_owned(),
        Value::String(bounded_text(model.get("name")?, MAX_DISPLAY_TEXT_BYTES)?),
    );
    insert_optional_text(
        &mut safe,
        "description",
        model.get("description"),
        MAX_DESCRIPTION_BYTES,
    )?;

    if let Some(metadata) = model.get("_meta").filter(|value| !value.is_null()) {
        let safe_metadata = safe_model_metadata(metadata.as_object()?)?;
        if !safe_metadata.is_empty() {
            safe.insert("_meta".to_owned(), Value::Object(safe_metadata));
        }
    }

    Some(Value::Object(safe))
}

fn safe_model_metadata(metadata: &Map<String, Value>) -> Option<Map<String, Value>> {
    let mut safe = Map::new();
    insert_optional_identity(
        &mut safe,
        "agentType",
        metadata.get("agentType"),
        MAX_PROTOCOL_TOKEN_BYTES,
    )?;
    let selected_reasoning_effort = match metadata.get("reasoningEffort") {
        None | Some(Value::Null) => None,
        Some(value) => Some(bounded_identity(value, MAX_PROTOCOL_TOKEN_BYTES)?),
    };
    if let Some(value) = selected_reasoning_effort.as_ref() {
        safe.insert("reasoningEffort".to_owned(), Value::String(value.clone()));
    }
    insert_optional_copy::<bool>(&mut safe, "supportsReasoningEffort", metadata)?;
    insert_optional_copy::<u64>(&mut safe, "totalContextTokens", metadata)?;

    if let Some(options) = metadata
        .get("reasoningEfforts")
        .filter(|value| !value.is_null())
    {
        let options = options.as_array()?;
        if options.len() > MAX_REASONING_EFFORTS {
            return None;
        }
        let mut ids = BTreeSet::new();
        let mut values = BTreeSet::new();
        let mut default_seen = false;
        let mut safe_options = Vec::with_capacity(options.len());
        for raw_option in options {
            let safe_option = safe_reasoning_effort_value(raw_option)?;
            let id = safe_option.get("id")?.as_str()?;
            let value = safe_option.get("value")?.as_str()?;
            if !ids.insert(id.to_owned()) || !values.insert(value.to_owned()) {
                return None;
            }
            if safe_option
                .get("default")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                if default_seen {
                    return None;
                }
                default_seen = true;
            }
            safe_options.push(safe_option);
        }
        if selected_reasoning_effort
            .as_ref()
            .is_some_and(|selected| !values.contains(selected))
        {
            return None;
        }
        safe.insert("reasoningEfforts".to_owned(), Value::Array(safe_options));
    } else if selected_reasoning_effort.is_some() {
        return None;
    }

    Some(safe)
}

fn safe_reasoning_effort_value(raw: &Value) -> Option<Value> {
    let option = raw.as_object()?;
    let mut safe = Map::new();
    safe.insert(
        "id".to_owned(),
        Value::String(bounded_identity(
            option.get("id")?,
            MAX_PROTOCOL_TOKEN_BYTES,
        )?),
    );
    safe.insert(
        "label".to_owned(),
        Value::String(bounded_text(option.get("label")?, MAX_DISPLAY_TEXT_BYTES)?),
    );
    safe.insert(
        "value".to_owned(),
        Value::String(bounded_identity(
            option.get("value")?,
            MAX_PROTOCOL_TOKEN_BYTES,
        )?),
    );
    insert_optional_text(
        &mut safe,
        "description",
        option.get("description"),
        MAX_DESCRIPTION_BYTES,
    )?;
    insert_optional_copy::<bool>(&mut safe, "default", option)?;
    Some(Value::Object(safe))
}

fn insert_optional_text(
    target: &mut Map<String, Value>,
    key: &str,
    value: Option<&Value>,
    max_bytes: usize,
) -> Option<()> {
    match value {
        None | Some(Value::Null) => {}
        Some(value) => {
            target.insert(
                key.to_owned(),
                Value::String(bounded_text(value, max_bytes)?),
            );
        }
    }
    Some(())
}

fn insert_optional_identity(
    target: &mut Map<String, Value>,
    key: &str,
    value: Option<&Value>,
    max_bytes: usize,
) -> Option<()> {
    match value {
        None | Some(Value::Null) => {}
        Some(value) => {
            target.insert(
                key.to_owned(),
                Value::String(bounded_identity(value, max_bytes)?),
            );
        }
    }
    Some(())
}

fn insert_optional_copy<T>(
    target: &mut Map<String, Value>,
    key: &str,
    source: &Map<String, Value>,
) -> Option<()>
where
    T: serde::de::DeserializeOwned + Serialize,
{
    match source.get(key) {
        None | Some(Value::Null) => {}
        Some(value) => {
            let value = serde_json::from_value::<T>(value.clone()).ok()?;
            target.insert(key.to_owned(), serde_json::to_value(value).ok()?);
        }
    }
    Some(())
}

fn find_session_config(metadata: &Value) -> Option<&Value> {
    select_consistent_alias([
        metadata.get("x.ai/sessionConfig"),
        metadata.get("_x.ai/sessionConfig"),
        metadata.get("sessionConfig"),
    ])
    .ok()
    .flatten()
}

fn normalize_config_options(config: &Value) -> Option<Vec<GrokSessionConfigOption>> {
    let options = config.get("options")?.as_array()?;
    if options.len() > MAX_CONFIG_OPTIONS {
        return None;
    }

    let mut ids = BTreeSet::new();
    let mut normalized = Vec::with_capacity(options.len());
    for raw in options {
        let option = normalize_config_option(raw)?;
        if !ids.insert(option.id.clone()) {
            return None;
        }
        normalized.push(option);
    }
    Some(normalized)
}

fn normalize_config_option(raw: &Value) -> Option<GrokSessionConfigOption> {
    let option = raw.as_object()?;
    Some(GrokSessionConfigOption {
        id: bounded_identity(option.get("id")?, MAX_IDENTIFIER_BYTES)?,
        label: bounded_text(option.get("label")?, MAX_DISPLAY_TEXT_BYTES)?,
        description: optional_bounded_text(option, "description", MAX_DESCRIPTION_BYTES)?,
        category: optional_bounded_identity(option, "category", MAX_PROTOCOL_TOKEN_BYTES)?,
        selected: optional_bool(option, "selected")?,
    })
}

fn optional_bounded_text(
    object: &Map<String, Value>,
    key: &str,
    max_bytes: usize,
) -> Option<Option<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(value) => bounded_text(value, max_bytes).map(Some),
    }
}

fn optional_bounded_identity(
    object: &Map<String, Value>,
    key: &str,
    max_bytes: usize,
) -> Option<Option<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(value) => bounded_identity(value, max_bytes).map(Some),
    }
}

fn optional_bool(object: &Map<String, Value>, key: &str) -> Option<Option<bool>> {
    match object.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(value) => value.as_bool().map(Some),
    }
}

fn bounded_text(value: &Value, max_bytes: usize) -> Option<String> {
    let raw = value.as_str()?;
    if raw.is_empty() {
        return None;
    }

    // Bound before any lowercasing or diagnostic scanning. The raw JSON value
    // already exists, but attacker-sized display text must not trigger another
    // attacker-sized allocation inside this normalization boundary.
    let bounded = bounded_owned(raw, max_bytes);
    let redacted = if has_sensitive_assignment(&bounded) {
        "[REDACTED]".to_owned()
    } else {
        redact_diagnostic(&bounded)
    };
    Some(bounded_owned(&redacted, max_bytes))
}

/// Keep identity-bearing values exact or reject them. Prefix truncation could
/// turn two distinct model/config identifiers into the same actionable value.
fn bounded_identity(value: &Value, max_bytes: usize) -> Option<String> {
    let raw = value.as_str()?;
    if raw.is_empty()
        || raw.len() > max_bytes
        || raw.chars().any(char::is_control)
        || has_sensitive_assignment(raw)
        || redact_diagnostic(raw) != raw
    {
        return None;
    }
    Some(raw.to_owned())
}

fn has_sensitive_assignment(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "credential",
        "authorization",
        "cookie",
        "api key",
        "api_key",
        "api-key",
        "apikey",
    ]
    .iter()
    .any(|label| {
        [':', '='].iter().any(|delimiter| {
            lower
                .match_indices(label)
                .any(|(start, _)| sensitive_label_has_value(&lower, start, label, *delimiter))
        })
    })
}

fn sensitive_label_has_value(value: &str, start: usize, label: &str, delimiter: char) -> bool {
    let before_is_boundary = value[..start]
        .chars()
        .next_back()
        .is_none_or(|character| !character.is_ascii_alphanumeric());
    if !before_is_boundary {
        return false;
    }

    let tail = value[start + label.len()..].trim_start();
    tail.strip_prefix(delimiter)
        .is_some_and(|assigned| !assigned.trim_start().is_empty())
}

fn bounded_owned(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }

    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn safe_summary_key(key: &str) -> Option<&str> {
    SAFE_RESPONSE_KEY_ALLOWLIST.contains(&key).then_some(key)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn observed_new_response_becomes_typed_without_private_metadata() {
        let secret = concat!("xai", "-1234567890abcdefghijklmnop");
        let response = json!({
            "jsonrpc": "2.0",
            "id": "private-request-id",
            "result": {
                "sessionId": "private-session-id",
                "models": {
                    "currentModelId": "grok-build",
                    "availableModels": [{
                        "modelId": "grok-build",
                        "name": "Grok Build",
                        "description": "Coding model",
                        "_meta": {
                            "agentType": "build",
                            "reasoningEffort": "high",
                            "reasoningEfforts": [{
                                "id": "high",
                                "label": "High",
                                "description": "More deliberate",
                                "value": "high",
                                "default": true,
                                "apiKey": secret
                            }],
                            "supportsReasoningEffort": true,
                            "totalContextTokens": 256000,
                            "credential": secret
                        }
                    }],
                    "futurePrivateValue": secret
                },
                "_meta": {
                    "currentWorkingDirectory": "C:\\Users\\Private\\repo",
                    "gitRoot": "C:\\Users\\Private\\repo",
                    "x.ai/sessionDetail": {
                        "cwd": "C:\\Users\\Private\\repo",
                        "transcript": "do not retain"
                    },
                    "x.ai/sessionConfig": {
                        "options": [{
                            "id": "grok-build",
                            "label": "Grok Build",
                            "description": "Current model",
                            "category": "model",
                            "selected": true,
                            "opaque": secret
                        }, {
                            "id": "high",
                            "label": "High effort",
                            "category": "mode",
                            "selected": true
                        }]
                    },
                    "apiKey": secret
                }
            }
        });

        let normalized = normalize_grok_session_response(response);
        let models = normalized.models.as_ref().expect("model catalog");
        assert_eq!(models.current_model_id, "grok-build");
        assert_eq!(models.available_models.len(), 1);
        assert_eq!(models.available_models[0].reasoning_efforts.len(), 1);
        let config_options = normalized.config_options.as_ref().expect("config options");
        assert_eq!(config_options.len(), 2);
        assert_eq!(config_options[0].category.as_deref(), Some("model"));
        assert_eq!(config_options[1].selected, Some(true));
        assert!(
            normalized
                .metadata_keys
                .as_slice()
                .contains(&"x.ai/sessionConfig".to_owned())
        );

        let serialized = serde_json::to_string(&normalized).unwrap();
        for private in [
            secret,
            "C:\\Users\\Private\\repo",
            "private-session-id",
            "private-request-id",
            "do not retain",
            "futurePrivateValue",
            "gitRoot",
            "sessionDetail",
            "apiKey",
            "opaque",
        ] {
            assert!(!serialized.contains(private), "retained {private:?}");
        }
    }

    #[test]
    fn empty_catalog_is_rejected_without_discarding_valid_wire_config() {
        let normalized = normalize_grok_session_response(json!({
            "_meta": {
                "modelState": {
                    "currentModelId": "fixture-model",
                    "availableModels": []
                },
                "_x.ai/sessionConfig": {
                    "options": [{
                        "id": "low",
                        "label": "Low",
                        "description": "Fast",
                        "category": "mode",
                        "selected": false
                    }]
                }
            }
        }));

        assert!(normalized.models.is_none());
        let config_options = normalized.config_options.expect("config options");
        assert_eq!(config_options[0].id, "low");
        assert_eq!(config_options[0].selected, Some(false));
    }

    #[test]
    fn current_model_without_available_models_is_not_a_complete_catalog() {
        let normalized = normalize_grok_session_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"models": {"currentModelId": "fixture-model"}}
        }));

        assert!(normalized.models.is_none());
    }

    #[test]
    fn malformed_json_rpc_result_cannot_activate_top_level_compatibility_fields() {
        let normalized = normalize_grok_session_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": false,
            "models": {
                "currentModelId": "fixture-model",
                "availableModels": [{"modelId": "fixture-model", "name": "Fixture"}]
            },
            "_meta": {
                "sessionConfig": {"options": [{"id": "mode", "label": "Mode"}]}
            }
        }));

        assert_eq!(normalized, GrokSessionResponse::default());

        let missing_result = normalize_grok_session_response(json!({
            "id": 1,
            "models": {
                "currentModelId": "fixture-model",
                "availableModels": [{"modelId": "fixture-model", "name": "Fixture"}]
            }
        }));
        assert_eq!(missing_result, GrokSessionResponse::default());

        let valid_catalog = json!({
            "models": {
                "currentModelId": "fixture-model",
                "availableModels": [{"modelId": "fixture-model", "name": "Fixture"}]
            }
        });
        let oversized_id = "x".repeat(MAX_RESPONSE_ID_BYTES + 1);
        for malformed in [
            json!({"jsonrpc": "1.0", "id": 1, "result": valid_catalog.clone()}),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": valid_catalog.clone(),
                "error": {"code": -1, "message": "conflict"}
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "session/new",
                "params": {},
                "result": valid_catalog.clone()
            }),
            json!({"jsonrpc": "2.0", "id": {}, "result": valid_catalog.clone()}),
            json!({"jsonrpc": "2.0", "id": true, "result": valid_catalog.clone()}),
            json!({"jsonrpc": "2.0", "id": 1.5, "result": valid_catalog.clone()}),
            json!({"jsonrpc": "2.0", "id": oversized_id, "result": valid_catalog.clone()}),
        ] {
            assert_eq!(
                normalize_grok_session_response(malformed),
                GrokSessionResponse::default()
            );
        }
    }

    #[test]
    fn model_state_aliases_reject_conflicts_but_accept_equal_or_null_duplicates() {
        let first = json!({
            "currentModelId": "first",
            "availableModels": [{"modelId": "first", "name": "First"}]
        });
        let second = json!({
            "currentModelId": "second",
            "availableModels": [{"modelId": "second", "name": "Second"}]
        });

        let conflicting = normalize_grok_session_response(json!({
            "models": first.clone(),
            "modelState": second
        }));
        assert!(conflicting.models.is_none());

        let equal = normalize_grok_session_response(json!({
            "models": first.clone(),
            "modelState": first.clone(),
            "_meta": {"models": first.clone()}
        }));
        assert_eq!(
            equal.models.expect("equal aliases").current_model_id,
            "first"
        );

        let equal_flattened = normalize_grok_session_response(json!({
            "models": first.clone(),
            "currentModelId": "first",
            "availableModels": [{"modelId": "first", "name": "First"}]
        }));
        assert_eq!(
            equal_flattened
                .models
                .expect("equal flattened alias")
                .current_model_id,
            "first"
        );

        let conflicting_flattened = normalize_grok_session_response(json!({
            "models": first.clone(),
            "currentModelId": "second",
            "availableModels": [{"modelId": "second", "name": "Second"}]
        }));
        assert!(conflicting_flattened.models.is_none());

        let incomplete_flattened = normalize_grok_session_response(json!({
            "models": first.clone(),
            "currentModelId": "first"
        }));
        assert!(incomplete_flattened.models.is_none());

        let null_and_value = normalize_grok_session_response(json!({
            "models": null,
            "modelState": first
        }));
        assert_eq!(
            null_and_value
                .models
                .expect("null alias must remain absent")
                .current_model_id,
            "first"
        );

        let nulls = normalize_grok_session_response(json!({
            "models": null,
            "modelState": null,
            "_meta": {"models": null, "modelState": null}
        }));
        assert!(nulls.models.is_none());
    }

    #[test]
    fn session_config_aliases_reject_conflicts_but_accept_equal_or_null_duplicates() {
        let first = json!({"options": [{"id": "first", "label": "First"}]});
        let second = json!({"options": [{"id": "second", "label": "Second"}]});

        let conflicting = normalize_grok_session_response(json!({
            "_meta": {
                "x.ai/sessionConfig": first.clone(),
                "_x.ai/sessionConfig": second
            }
        }));
        assert!(conflicting.config_options.is_none());

        let equal = normalize_grok_session_response(json!({
            "_meta": {
                "x.ai/sessionConfig": first.clone(),
                "_x.ai/sessionConfig": first.clone(),
                "sessionConfig": first.clone()
            }
        }));
        assert_eq!(equal.config_options.expect("equal aliases")[0].id, "first");

        let null_and_value = normalize_grok_session_response(json!({
            "_meta": {
                "x.ai/sessionConfig": null,
                "sessionConfig": first
            }
        }));
        assert_eq!(
            null_and_value
                .config_options
                .expect("null alias must remain absent")[0]
                .id,
            "first"
        );

        let nulls = normalize_grok_session_response(json!({
            "_meta": {
                "x.ai/sessionConfig": null,
                "_x.ai/sessionConfig": null,
                "sessionConfig": null
            }
        }));
        assert!(nulls.config_options.is_none());
    }

    #[test]
    fn malformed_and_opaque_values_are_omitted_with_only_safe_keys() {
        let normalized = normalize_grok_session_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "models": {"currentModelId": 7, "availableModels": "oops"},
                "future-safe": {"nested": "not retained"},
                "cwd": "D:\\private\\repo",
                "access_token": "secret",
                "_meta": {
                    "x.ai/sessionConfig": {"options": [
                        {"id": "missing-label"},
                        "not-an-object"
                    ]},
                    "gitRoot": "D:\\private\\repo",
                    "future.flag": "not retained",
                    "privateNotes": "not retained"
                }
            }
        }));

        assert!(normalized.models.is_none());
        assert!(normalized.config_options.is_none());
        assert_eq!(
            normalized.response_keys.as_slice(),
            &[
                "<unknown-key>".to_owned(),
                "_meta".to_owned(),
                "models".to_owned()
            ]
        );
        assert_eq!(
            normalized.metadata_keys.as_slice(),
            &["<unknown-key>".to_owned(), "x.ai/sessionConfig".to_owned()]
        );

        let serialized = serde_json::to_string(&normalized).unwrap();
        assert!(!serialized.contains("D:\\\\private"));
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("not retained"));
        assert!(!serialized.contains("gitRoot"));
        assert!(!serialized.contains("privateNotes"));
    }

    #[test]
    fn arbitrary_object_keys_collapse_without_leaking_key_text() {
        let alphanumeric_secret = "skliveABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
        let uuid_key = "019d1234-5678-7abc-8def-0123456789ab";
        let path_key = "C:/Users/Private/project";
        let normalized = normalize_grok_session_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "models": {"currentModelId": "fixture-model"},
                (alphanumeric_secret): true,
                (uuid_key): true,
                "_meta": {
                    "x.ai/sessionConfig": {"options": []},
                    (path_key): true
                }
            }
        }));

        assert!(
            normalized
                .response_keys
                .as_slice()
                .contains(&"<unknown-key>".to_owned())
        );
        assert!(
            normalized
                .metadata_keys
                .as_slice()
                .contains(&"<unknown-key>".to_owned())
        );
        let serialized = serde_json::to_string(&normalized).unwrap();
        assert!(!serialized.contains(alphanumeric_secret));
        assert!(!serialized.contains(uuid_key));
        assert!(!serialized.contains(path_key));
    }

    #[test]
    fn oversized_identity_or_state_collections_are_rejected_without_collisions() {
        let long = "🦀".repeat(1_000);
        let models: Vec<Value> = (0..(MAX_MODELS + 8))
            .map(|index| {
                json!({
                    "modelId": format!("model-{index}"),
                    "name": long,
                    "_meta": {
                        "reasoningEfforts": (0..(MAX_REASONING_EFFORTS + 4))
                            .map(|effort| json!({
                                "id": format!("effort-{effort}"),
                                "label": long,
                                "value": format!("effort-{effort}")
                            }))
                            .collect::<Vec<_>>()
                    }
                })
            })
            .collect();
        let options: Vec<Value> = (0..(MAX_CONFIG_OPTIONS + 8))
            .map(|index| json!({"id": format!("id-{index}"), "label": long}))
            .collect();

        let normalized = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": long,
                "availableModels": models
            },
            "_meta": {"x.ai/sessionConfig": {"options": options}}
        }));

        assert!(normalized.models.is_none());
        assert!(normalized.config_options.is_none());

        let shared_prefix = "m".repeat(MAX_IDENTIFIER_BYTES);
        for suffix in ['a', 'b'] {
            let normalized = normalize_grok_session_response(json!({
                "models": {"currentModelId": format!("{shared_prefix}{suffix}")}
            }));
            assert!(normalized.models.is_none());
        }
    }

    #[test]
    fn display_text_is_utf8_bounded_while_actionable_ids_remain_exact() {
        let long = "\u{1F980}".repeat(1_000);
        let normalized = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "fixture-model",
                "availableModels": [{
                    "modelId": "fixture-model",
                    "name": long,
                    "_meta": {"reasoningEfforts": [{
                        "id": "high",
                        "label": long,
                        "value": "high"
                    }]}
                }]
            },
            "_meta": {"x.ai/sessionConfig": {"options": [{
                "id": "high",
                "label": long
            }]}}
        }));

        let models = normalized.models.expect("model state");
        assert_eq!(models.current_model_id, "fixture-model");
        assert_eq!(
            models.available_models[0].name.len(),
            MAX_DISPLAY_TEXT_BYTES
        );
        assert!(
            models.available_models[0]
                .name
                .is_char_boundary(models.available_models[0].name.len())
        );
        let options = normalized.config_options.expect("config options");
        assert_eq!(options[0].id, "high");
        assert_eq!(options[0].label.len(), MAX_DISPLAY_TEXT_BYTES);
    }

    #[test]
    fn nested_oversized_actionable_ids_reject_the_complete_snapshot() {
        let oversized = format!("{}x", "i".repeat(MAX_IDENTIFIER_BYTES));
        let models = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "fixture-model",
                "availableModels": [{
                    "modelId": oversized.clone(),
                    "name": "Unsafe prefix collision"
                }]
            }
        }));
        assert!(models.models.is_none());

        let reasoning = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "fixture-model",
                "availableModels": [{
                    "modelId": "fixture-model",
                    "name": "Fixture",
                    "_meta": {"reasoningEfforts": [{
                        "id": "high",
                        "label": "High",
                        "value": oversized.clone()
                    }]}
                }]
            }
        }));
        assert!(reasoning.models.is_none());

        let config = normalize_grok_session_response(json!({
            "_meta": {"x.ai/sessionConfig": {"options": [{
                "id": oversized,
                "label": "Unsafe prefix collision"
            }]}}
        }));
        assert!(config.config_options.is_none());
    }

    #[test]
    fn complete_catalog_rejects_duplicate_or_unresolved_model_ids() {
        let duplicate = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "model-a",
                "availableModels": [
                    {"modelId": "model-a", "name": "Model A"},
                    {"modelId": "model-a", "name": "Model A duplicate"}
                ]
            }
        }));
        assert!(duplicate.models.is_none());

        let unresolved = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "model-missing",
                "availableModels": [
                    {"modelId": "model-a", "name": "Model A"},
                    {"modelId": "model-b", "name": "Model B"}
                ]
            }
        }));
        assert!(unresolved.models.is_none());
    }

    #[test]
    fn duplicate_reasoning_ids_or_values_reject_the_complete_catalog() {
        for reasoning_efforts in [
            json!([
                {"id": "high", "label": "High", "value": "high"},
                {"id": "high", "label": "High duplicate", "value": "max"}
            ]),
            json!([
                {"id": "high", "label": "High", "value": "high"},
                {"id": "max", "label": "Max alias", "value": "high"}
            ]),
        ] {
            let normalized = normalize_grok_session_response(json!({
                "models": {
                    "currentModelId": "fixture-model",
                    "availableModels": [{
                        "modelId": "fixture-model",
                        "name": "Fixture",
                        "_meta": {"reasoningEfforts": reasoning_efforts}
                    }]
                }
            }));
            assert!(normalized.models.is_none());
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
            let normalized = normalize_grok_session_response(json!({
                "models": {
                    "currentModelId": "fixture-model",
                    "availableModels": [{
                        "modelId": "fixture-model",
                        "name": "Fixture",
                        "_meta": metadata
                    }]
                }
            }));
            assert!(normalized.models.is_none());
        }

        let valid = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "fixture-model",
                "availableModels": [{
                    "modelId": "fixture-model",
                    "name": "Fixture",
                    "_meta": {
                        "reasoningEffort": "high",
                        "reasoningEfforts": [
                            {"id": "low", "label": "Low", "value": "low"},
                            {"id": "high", "label": "High", "value": "high", "default": true}
                        ]
                    }
                }]
            }
        }));
        let model = &valid
            .models
            .expect("consistent reasoning snapshot")
            .available_models[0];
        assert_eq!(model.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            model
                .reasoning_efforts
                .iter()
                .filter(|option| option.is_default)
                .count(),
            1
        );

        let nulls = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "fixture-model",
                "availableModels": [{
                    "modelId": "fixture-model",
                    "name": "Fixture",
                    "_meta": {"reasoningEffort": null, "reasoningEfforts": null}
                }]
            }
        }));
        assert!(nulls.models.is_some());
    }

    #[test]
    fn wrong_typed_optional_model_fields_reject_the_complete_catalog() {
        let malformed_models = [
            json!({
                "modelId": "fixture-model",
                "name": "Fixture",
                "description": false
            }),
            json!({
                "modelId": "fixture-model",
                "name": "Fixture",
                "_meta": {"supportsReasoningEffort": "yes"}
            }),
            json!({
                "modelId": "fixture-model",
                "name": "Fixture",
                "_meta": {"totalContextTokens": -1}
            }),
            json!({
                "modelId": "fixture-model",
                "name": "Fixture",
                "_meta": {"reasoningEfforts": [{
                    "id": "high",
                    "label": "High",
                    "value": "high",
                    "default": "yes"
                }]}
            }),
        ];

        for malformed_model in malformed_models {
            let normalized = normalize_grok_session_response(json!({
                "models": {
                    "currentModelId": "fixture-model",
                    "availableModels": [malformed_model]
                }
            }));
            assert!(
                normalized.models.is_none(),
                "accepted a wrong-typed optional model field"
            );
        }
    }

    #[test]
    fn malformed_optional_config_fields_or_duplicate_ids_reject_the_snapshot() {
        for (key, malformed) in [
            ("description", json!(false)),
            ("category", json!(["mode"])),
            ("selected", json!("yes")),
        ] {
            let mut option = json!({"id": "high", "label": "High"});
            option
                .as_object_mut()
                .expect("test option is an object")
                .insert(key.to_owned(), malformed);
            let normalized = normalize_grok_session_response(json!({
                "_meta": {"x.ai/sessionConfig": {"options": [option]}}
            }));
            assert!(
                normalized.config_options.is_none(),
                "accepted malformed {key}"
            );
        }

        let duplicate = normalize_grok_session_response(json!({
            "_meta": {"x.ai/sessionConfig": {"options": [
                {"id": "high", "label": "High"},
                {"id": "high", "label": "High duplicate"}
            ]}}
        }));
        assert!(duplicate.config_options.is_none());
    }

    #[test]
    fn null_optional_model_and_config_fields_remain_absent() {
        let normalized = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "model-null-meta",
                "availableModels": [
                    {
                        "modelId": "model-null-meta",
                        "name": "Null metadata",
                        "description": null,
                        "_meta": null
                    },
                    {
                        "modelId": "model-null-fields",
                        "name": "Null fields",
                        "_meta": {
                            "agentType": null,
                            "reasoningEffort": null,
                            "reasoningEfforts": null,
                            "supportsReasoningEffort": null,
                            "totalContextTokens": null
                        }
                    }
                ]
            },
            "_meta": {"x.ai/sessionConfig": {"options": [{
                "id": "high",
                "label": "High",
                "description": null,
                "category": null,
                "selected": null
            }]}}
        }));

        let catalog = normalized.models.expect("null optional model fields");
        assert_eq!(catalog.available_models.len(), 2);
        assert!(catalog.available_models[0].description.is_none());
        assert!(catalog.available_models[1].agent_type.is_none());
        assert!(catalog.available_models[1].reasoning_effort.is_none());
        assert!(catalog.available_models[1].reasoning_efforts.is_empty());
        assert!(
            catalog.available_models[1]
                .supports_reasoning_effort
                .is_none()
        );
        assert!(catalog.available_models[1].total_context_tokens.is_none());

        let option = &normalized.config_options.expect("null config fields")[0];
        assert!(option.description.is_none());
        assert!(option.category.is_none());
        assert!(option.selected.is_none());
    }

    #[test]
    fn model_catalog_accepts_the_same_exact_collection_limits_as_updates() {
        let reasoning_efforts = (0..MAX_REASONING_EFFORTS)
            .map(|index| {
                json!({
                    "id": format!("effort-{index}"),
                    "label": format!("Effort {index}"),
                    "value": format!("effort-{index}")
                })
            })
            .collect::<Vec<_>>();
        let models = (0..MAX_MODELS)
            .map(|index| {
                if index == 0 {
                    json!({
                        "modelId": format!("model-{index}"),
                        "name": format!("Model {index}"),
                        "_meta": {"reasoningEfforts": reasoning_efforts.clone()}
                    })
                } else {
                    json!({
                        "modelId": format!("model-{index}"),
                        "name": format!("Model {index}")
                    })
                }
            })
            .collect::<Vec<_>>();

        let normalized = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "model-0",
                "availableModels": models
            }
        }));
        let catalog = normalized.models.expect("catalog at exact shared limits");
        assert_eq!(catalog.available_models.len(), MAX_MODELS);
        assert_eq!(
            catalog.available_models[0].reasoning_efforts.len(),
            MAX_REASONING_EFFORTS
        );
    }

    #[test]
    fn model_catalog_accepts_the_same_exact_scalar_limits_as_updates() {
        let model_id = "m".repeat(MAX_IDENTIFIER_BYTES);
        let effort_id = "e".repeat(MAX_PROTOCOL_TOKEN_BYTES);
        let effort_value = "v".repeat(MAX_PROTOCOL_TOKEN_BYTES);
        let agent_type = "a".repeat(MAX_PROTOCOL_TOKEN_BYTES);
        let name = "n".repeat(MAX_DISPLAY_TEXT_BYTES);
        let description = "d".repeat(MAX_DESCRIPTION_BYTES);
        let normalized = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": model_id,
                "availableModels": [{
                    "modelId": model_id,
                    "name": name,
                    "description": description,
                    "_meta": {
                        "agentType": agent_type,
                        "reasoningEffort": effort_value,
                        "reasoningEfforts": [{
                            "id": effort_id,
                            "label": name,
                            "description": description,
                            "value": effort_value
                        }]
                    }
                }]
            }
        }));

        let catalog = normalized.models.expect("catalog at exact shared limits");
        let model = &catalog.available_models[0];
        assert_eq!(model.model_id.len(), MAX_IDENTIFIER_BYTES);
        assert_eq!(model.name.len(), MAX_DISPLAY_TEXT_BYTES);
        assert_eq!(
            model.description.as_ref().map(String::len),
            Some(MAX_DESCRIPTION_BYTES)
        );
        assert_eq!(
            model.agent_type.as_ref().map(String::len),
            Some(MAX_PROTOCOL_TOKEN_BYTES)
        );
        assert_eq!(
            model.reasoning_efforts[0].id.len(),
            MAX_PROTOCOL_TOKEN_BYTES
        );
        assert_eq!(
            model.reasoning_efforts[0].value.len(),
            MAX_PROTOCOL_TOKEN_BYTES
        );
    }

    #[test]
    fn unknown_keys_cannot_crow_reviewed_keys_out_of_a_summary() {
        let mut object = Map::new();
        for key in SAFE_RESPONSE_KEY_ALLOWLIST {
            object.insert((*key).to_owned(), Value::Null);
        }
        for index in 0..(MAX_SUMMARY_KEYS * 4) {
            object.insert(format!("!unknown-{index:03}"), Value::Null);
        }

        let summary = SafeResponseKeys::from_value(&Value::Object(object));
        assert_eq!(summary.as_slice().len(), MAX_SUMMARY_KEYS);
        assert!(
            SAFE_RESPONSE_KEY_ALLOWLIST
                .iter()
                .all(|key| summary.as_slice().iter().any(|safe| safe == key))
        );
        assert_eq!(
            summary
                .as_slice()
                .iter()
                .filter(|key| key.as_str() == UNKNOWN_RESPONSE_KEY)
                .count(),
            1
        );
    }

    #[test]
    fn oversized_display_text_is_bounded_before_sensitive_scanning() {
        let mut raw = "a".repeat(MAX_DISPLAY_TEXT_BYTES);
        raw.push_str(&" token: far-away-secret".repeat(50_000));
        let value = Value::String(raw);

        let normalized = bounded_text(&value, MAX_DISPLAY_TEXT_BYTES).expect("display text");
        assert_eq!(normalized, "a".repeat(MAX_DISPLAY_TEXT_BYTES));
        assert_eq!(normalized.len(), MAX_DISPLAY_TEXT_BYTES);
    }

    #[test]
    fn credentials_in_known_display_fields_are_redacted() {
        let provider_key = concat!("xai", "-1234567890abcdefghijklmnop");
        let normalized = normalize_grok_session_response(json!({
            "models": {
                "currentModelId": "grok-build",
                "availableModels": [{
                    "modelId": "grok-build",
                    "name": "Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
                    "description": format!("api_key={provider_key}")
                }]
            },
            "_meta": {"x.ai/sessionConfig": {"options": [{
                "id": "high",
                "label": "token: super-secret-value",
                "description": "Bearer abcdefghijklmnopqrstuvwxyz"
            }]}}
        }));

        let serialized = serde_json::to_string(&normalized).unwrap();
        assert!(serialized.contains("[REDACTED]"));
        assert!(!serialized.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!serialized.contains(provider_key));
        assert!(!serialized.contains("super-secret-value"));
    }
}
