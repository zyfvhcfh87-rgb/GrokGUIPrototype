use agent_client_protocol::JsonRpcRequest;
use agent_client_protocol::schema::v1::InitializeResponse;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CurrentModelSelection {
    pub model_id: String,
    pub reasoning_effort: Option<String>,
}

pub(crate) fn response_confirms_model(response: &Value, expected_model: &str) -> bool {
    response.pointer("/_meta/model").and_then(|model| {
        model
            .as_str()
            .or_else(|| model.get("Ok").and_then(Value::as_str))
    }) == Some(expected_model)
}

impl CurrentModelSelection {
    pub(crate) fn from_initialize(response: &InitializeResponse) -> Option<Self> {
        let model_state = response.meta.as_ref()?.get("modelState")?.as_object()?;
        let model_id = bounded_string(model_state.get("currentModelId")?)?;
        let model = model_state
            .get("availableModels")?
            .as_array()?
            .iter()
            .find(|model| model.get("modelId").and_then(Value::as_str) == Some(model_id.as_str()));
        let reasoning_effort = model
            .and_then(|model| model.get("_meta"))
            .and_then(|meta| meta.get("reasoningEffort"))
            .and_then(bounded_string);

        Some(Self {
            model_id,
            reasoning_effort,
        })
    }

    pub(crate) fn into_request(self, session_id: impl Into<String>) -> LegacySetModelRequest {
        LegacySetModelRequest::new(session_id, self.model_id, self.reasoning_effort)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonRpcRequest)]
#[request(method = "session/set_model", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacySetModelRequest {
    session_id: String,
    model_id: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    meta: Option<Map<String, Value>>,
}

impl LegacySetModelRequest {
    fn new(
        session_id: impl Into<String>,
        model_id: impl Into<String>,
        reasoning_effort: Option<String>,
    ) -> Self {
        let meta = reasoning_effort.map(|reasoning_effort| {
            Map::from_iter([(
                "reasoningEffort".to_owned(),
                Value::String(reasoning_effort),
            )])
        });
        Self {
            session_id: session_id.into(),
            model_id: model_id.into(),
            meta,
        }
    }
}

fn bounded_string(value: &Value) -> Option<String> {
    const MAX_METADATA_VALUE_BYTES: usize = 256;
    value
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= MAX_METADATA_VALUE_BYTES)
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::ProtocolVersion;
    use serde_json::json;

    use super::*;

    #[test]
    fn selects_only_the_current_advertised_model_and_effort() {
        let mut meta = Map::new();
        meta.insert(
            "modelState".to_owned(),
            json!({
                "currentModelId": "fixture-model",
                "availableModels": [
                    {
                        "modelId": "other-model",
                        "_meta": { "reasoningEffort": "low" }
                    },
                    {
                        "modelId": "fixture-model",
                        "name": "Fixture model",
                        "_meta": {
                            "reasoningEffort": "high",
                            "privateFutureField": "must-not-be-retained"
                        }
                    }
                ],
                "privateFutureField": "must-not-be-retained"
            }),
        );
        let response = InitializeResponse::new(ProtocolVersion::V1).meta(meta);

        let selection = CurrentModelSelection::from_initialize(&response).unwrap();

        assert_eq!(selection.model_id, "fixture-model");
        assert_eq!(selection.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn legacy_request_matches_grok_wire_shape_without_extra_metadata() {
        let request =
            LegacySetModelRequest::new("session-001", "fixture-model", Some("high".to_owned()));

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "sessionId": "session-001",
                "modelId": "fixture-model",
                "_meta": { "reasoningEffort": "high" }
            })
        );
    }

    #[test]
    fn missing_model_state_fails_closed() {
        let response = InitializeResponse::new(ProtocolVersion::V1);
        assert!(CurrentModelSelection::from_initialize(&response).is_none());
    }

    #[test]
    fn accepts_both_source_and_installed_model_response_shapes() {
        assert!(response_confirms_model(
            &json!({ "_meta": { "model": "fixture-model" } }),
            "fixture-model"
        ));
        assert!(response_confirms_model(
            &json!({ "_meta": { "model": { "Ok": "fixture-model" } } }),
            "fixture-model"
        ));
        assert!(!response_confirms_model(
            &json!({ "_meta": { "model": { "Ok": "other-model" } } }),
            "fixture-model"
        ));
    }
}
