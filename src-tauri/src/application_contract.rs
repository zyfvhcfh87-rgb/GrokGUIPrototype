use std::sync::Mutex;

use serde::{Deserialize, Serialize};

const MAX_EVENT_TEXT_BYTES: usize = 64 * 1_024;
const MAX_DISPLAY_TEXT_BYTES: usize = 8 * 1_024;
const MAX_EVENT_COLLECTION_ITEMS: usize = 256;
const MAX_WORKSPACE_PATH_BYTES: usize = 32 * 1_024;
const MAX_EXTERNAL_URL_BYTES: usize = 2_048;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_CURSOR_BYTES: usize = 4 * 1_024;
const MAX_PROMPT_BYTES: usize = 1_024 * 1_024;
const MAX_ELICITATION_FIELDS: usize = 64;
const MAX_ELICITATION_VALUE_BYTES: usize = 16 * 1_024;
const MAX_ELICITATION_ARRAY_ITEMS: usize = 64;
const MAX_SERIALIZED_EVENT_BYTES: usize = 8 * 1_024 * 1_024;
const TRUNCATED_SUFFIX: &str = " [TRUNCATED]";

pub const APPLICATION_EVENT_NAME: &str = "grok-application-event";
#[cfg(test)]
pub const APPLICATION_COMMAND_NAMES: &[&str] = &[
    "setup_status",
    "workspace_pick",
    "workspace_validate",
    "workspace_recent_list",
    "workspace_recent_remove",
    "open_external_url",
    "runtime_snapshot",
    "runtime_start",
    "runtime_stop",
    "runtime_restart",
    "session_new",
    "session_list",
    "session_load",
    "session_resume",
    "session_close",
    "prompt_send",
    "prompt_cancel",
    "session_set_mode",
    "session_set_model",
    "session_set_config",
    "permission_respond",
    "elicitation_respond",
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRequestDto {
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenExternalUrlRequestDto {
    pub url: String,
}

impl OpenExternalUrlRequestDto {
    pub fn validated_url(self) -> Result<String, ApplicationErrorDto> {
        validate_external_url(&self.url)
    }
}

impl WorkspaceRequestDto {
    pub fn into_path(self) -> Result<std::path::PathBuf, ApplicationErrorDto> {
        if self.path.is_empty()
            || self.path.len() > MAX_WORKSPACE_PATH_BYTES
            || self.path.chars().any(char::is_control)
        {
            return Err(ApplicationErrorDto::invalid_workspace());
        }
        Ok(std::path::PathBuf::from(self.path))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDto {
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentWorkspaceDto {
    pub path: String,
    pub available: bool,
    pub last_session_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentWorkspaceListDto {
    pub workspaces: Vec<RecentWorkspaceDto>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionRequestDto {
    pub workspace: String,
}

impl NewSessionRequestDto {
    pub fn into_runtime_command(self) -> Result<grok_runtime::RuntimeCommand, ApplicationErrorDto> {
        validate_workspace_input(&self.workspace)?;
        Ok(grok_runtime::RuntimeCommand::NewSession {
            workspace: std::path::PathBuf::from(self.workspace),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsRequestDto {
    pub workspace: Option<String>,
    pub cursor: Option<String>,
}

impl ListSessionsRequestDto {
    pub fn into_runtime_command(self) -> Result<grok_runtime::RuntimeCommand, ApplicationErrorDto> {
        if let Some(workspace) = self.workspace.as_deref() {
            validate_workspace_input(workspace)?;
        }
        if let Some(cursor) = self.cursor.as_deref() {
            validate_token(cursor, MAX_CURSOR_BYTES)?;
        }
        Ok(grok_runtime::RuntimeCommand::ListSessions {
            workspace: self.workspace.map(std::path::PathBuf::from),
            cursor: self.cursor,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWorkspaceRequestDto {
    pub session_id: String,
    pub workspace: String,
}

impl SessionWorkspaceRequestDto {
    pub fn into_load_command(self) -> Result<grok_runtime::RuntimeCommand, ApplicationErrorDto> {
        validate_token(&self.session_id, MAX_IDENTIFIER_BYTES)?;
        validate_workspace_input(&self.workspace)?;
        Ok(grok_runtime::RuntimeCommand::LoadSession {
            session_id: self.session_id,
            workspace: std::path::PathBuf::from(self.workspace),
        })
    }

    pub fn into_resume_command(self) -> Result<grok_runtime::RuntimeCommand, ApplicationErrorDto> {
        validate_token(&self.session_id, MAX_IDENTIFIER_BYTES)?;
        validate_workspace_input(&self.workspace)?;
        Ok(grok_runtime::RuntimeCommand::ResumeSession {
            session_id: self.session_id,
            workspace: std::path::PathBuf::from(self.workspace),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequestDto {
    pub session_id: String,
}

impl SessionRequestDto {
    pub fn validated_session_id(self) -> Result<String, ApplicationErrorDto> {
        validate_token(&self.session_id, MAX_IDENTIFIER_BYTES)?;
        Ok(self.session_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequestDto {
    pub session_id: String,
    pub text: String,
}

impl PromptRequestDto {
    pub fn into_runtime_command(self) -> Result<grok_runtime::RuntimeCommand, ApplicationErrorDto> {
        validate_token(&self.session_id, MAX_IDENTIFIER_BYTES)?;
        if self.text.is_empty() || self.text.len() > MAX_PROMPT_BYTES {
            return Err(ApplicationErrorDto::invalid_request());
        }
        Ok(grok_runtime::RuntimeCommand::Prompt {
            session_id: self.session_id,
            text: self.text,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionModeRequestDto {
    pub session_id: String,
    pub mode_id: String,
}

impl SetSessionModeRequestDto {
    pub fn into_runtime_command(self) -> Result<grok_runtime::RuntimeCommand, ApplicationErrorDto> {
        validate_token(&self.session_id, MAX_IDENTIFIER_BYTES)?;
        validate_token(&self.mode_id, MAX_IDENTIFIER_BYTES)?;
        Ok(grok_runtime::RuntimeCommand::SetSessionMode {
            session_id: self.session_id,
            mode_id: self.mode_id,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionModelRequestDto {
    pub session_id: String,
    pub model_id: String,
    pub reasoning_effort: Option<String>,
}

impl SetSessionModelRequestDto {
    pub fn into_runtime_command(self) -> Result<grok_runtime::RuntimeCommand, ApplicationErrorDto> {
        validate_token(&self.session_id, MAX_IDENTIFIER_BYTES)?;
        validate_token(&self.model_id, MAX_IDENTIFIER_BYTES)?;
        if let Some(effort) = self.reasoning_effort.as_deref() {
            validate_token(effort, MAX_IDENTIFIER_BYTES)?;
        }
        Ok(grok_runtime::RuntimeCommand::SetSessionModel {
            session_id: self.session_id,
            model_id: self.model_id,
            reasoning_effort: self.reasoning_effort,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionConfigRequestDto {
    pub session_id: String,
    pub config_id: String,
    pub value: ConfigValueDto,
}

impl SetSessionConfigRequestDto {
    pub fn into_runtime_command(self) -> Result<grok_runtime::RuntimeCommand, ApplicationErrorDto> {
        validate_token(&self.session_id, MAX_IDENTIFIER_BYTES)?;
        validate_token(&self.config_id, MAX_IDENTIFIER_BYTES)?;
        if let ConfigValueDto::Select(value) = &self.value {
            validate_token(value, MAX_IDENTIFIER_BYTES)?;
        }
        Ok(grok_runtime::RuntimeCommand::SetSessionConfig {
            session_id: self.session_id,
            config_id: self.config_id,
            value: self.value.into(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ConfigValueDto {
    Select(String),
    Boolean(bool),
}

impl From<ConfigValueDto> for grok_runtime::RuntimeConfigValue {
    fn from(value: ConfigValueDto) -> Self {
        match value {
            ConfigValueDto::Select(value) => Self::Select(value),
            ConfigValueDto::Boolean(value) => Self::Boolean(value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionResponseRequestDto {
    pub interaction_id: String,
    pub decision: PermissionDecisionDto,
}

impl PermissionResponseRequestDto {
    pub fn validate(&self) -> Result<(), ApplicationErrorDto> {
        validate_token(&self.interaction_id, MAX_IDENTIFIER_BYTES)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecisionDto {
    AllowOnce,
    AllowAlways,
    DenyOnce,
    DenyAlways,
    Cancel,
}

impl From<PermissionDecisionDto> for grok_runtime::PermissionDecision {
    fn from(value: PermissionDecisionDto) -> Self {
        match value {
            PermissionDecisionDto::AllowOnce => Self::AllowOnce,
            PermissionDecisionDto::AllowAlways => Self::AllowAlways,
            PermissionDecisionDto::DenyOnce => Self::DenyOnce,
            PermissionDecisionDto::DenyAlways => Self::DenyAlways,
            PermissionDecisionDto::Cancel => Self::Cancel,
        }
    }
}

impl From<grok_runtime::PermissionDecision> for PermissionDecisionDto {
    fn from(value: grok_runtime::PermissionDecision) -> Self {
        match value {
            grok_runtime::PermissionDecision::AllowOnce => Self::AllowOnce,
            grok_runtime::PermissionDecision::AllowAlways => Self::AllowAlways,
            grok_runtime::PermissionDecision::DenyOnce => Self::DenyOnce,
            grok_runtime::PermissionDecision::DenyAlways => Self::DenyAlways,
            grok_runtime::PermissionDecision::Cancel => Self::Cancel,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationResponseRequestDto {
    pub interaction_id: String,
    pub decision: ElicitationDecisionDto,
}

impl ElicitationResponseRequestDto {
    pub fn validate(&self) -> Result<(), ApplicationErrorDto> {
        validate_token(&self.interaction_id, MAX_IDENTIFIER_BYTES)?;
        let ElicitationDecisionDto::Accept { content } = &self.decision else {
            return Ok(());
        };
        if content.len() > MAX_ELICITATION_FIELDS {
            return Err(ApplicationErrorDto::invalid_request());
        }
        for (key, value) in content {
            validate_token(key, MAX_IDENTIFIER_BYTES)?;
            match value {
                ElicitationValueDto::String(value) => validate_elicitation_text(value)?,
                ElicitationValueDto::Number(value) if !value.is_finite() => {
                    return Err(ApplicationErrorDto::invalid_request());
                }
                ElicitationValueDto::StringArray(values) => {
                    if values.len() > MAX_ELICITATION_ARRAY_ITEMS {
                        return Err(ApplicationErrorDto::invalid_request());
                    }
                    for value in values {
                        validate_elicitation_text(value)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ElicitationDecisionDto {
    Accept {
        content: std::collections::BTreeMap<String, ElicitationValueDto>,
    },
    Decline,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ElicitationValueDto {
    String(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    StringArray(Vec<String>),
}

impl From<ElicitationDecisionDto> for grok_runtime::ElicitationDecision {
    fn from(value: ElicitationDecisionDto) -> Self {
        match value {
            ElicitationDecisionDto::Accept { content } => Self::Accept {
                content: content
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            },
            ElicitationDecisionDto::Decline => Self::Decline,
            ElicitationDecisionDto::Cancel => Self::Cancel,
        }
    }
}

impl From<ElicitationValueDto> for grok_runtime::ElicitationValue {
    fn from(value: ElicitationValueDto) -> Self {
        match value {
            ElicitationValueDto::String(value) => Self::String(value),
            ElicitationValueDto::Integer(value) => Self::Integer(value),
            ElicitationValueDto::Number(value) => Self::Number(value),
            ElicitationValueDto::Boolean(value) => Self::Boolean(value),
            ElicitationValueDto::StringArray(value) => Self::StringArray(value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatusDto {
    pub runtime_available: bool,
    pub executable_state: ExecutableStateDto,
    pub executable_source: Option<ExecutableSourceDto>,
    pub failure: Option<ApplicationErrorDto>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutableStateDto {
    Available,
    Missing,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutableSourceDto {
    Configured,
    UserInstall,
    Path,
}

impl From<grok_runtime::GrokExecutableSource> for ExecutableSourceDto {
    fn from(value: grok_runtime::GrokExecutableSource) -> Self {
        match value {
            grok_runtime::GrokExecutableSource::Environment => Self::Configured,
            grok_runtime::GrokExecutableSource::UserInstall => Self::UserInstall,
            grok_runtime::GrokExecutableSource::Path => Self::Path,
            grok_runtime::GrokExecutableSource::Explicit => Self::Configured,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshotDto {
    pub generation: u64,
    pub last_sequence: u64,
    pub state: ApplicationRuntimeState,
    pub capabilities: Option<RuntimeCapabilitiesDto>,
}

impl RuntimeSnapshotDto {
    pub fn from_runtime(
        snapshot: grok_runtime::RuntimeSnapshot,
        checkpoint: ApplicationEventCheckpoint,
    ) -> Self {
        Self {
            generation: checkpoint.generation,
            last_sequence: checkpoint.sequence,
            state: checkpoint.runtime_state,
            capabilities: snapshot.capabilities.map(RuntimeCapabilitiesDto::from),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCapabilitiesDto {
    pub protocol_version: u16,
    pub agent: RuntimeAgentDto,
    pub authentication_methods: Vec<AuthenticationMethodDto>,
    pub sessions: RuntimeSessionCapabilitiesDto,
    pub models: Option<ModelCatalogDto>,
    pub truncated: bool,
}

impl From<grok_runtime::RuntimeCapabilities> for RuntimeCapabilitiesDto {
    fn from(value: grok_runtime::RuntimeCapabilities) -> Self {
        let truncated = value.authentication_methods.len() > MAX_EVENT_COLLECTION_ITEMS;
        Self {
            protocol_version: value.protocol_version,
            agent: value.agent.into(),
            authentication_methods: value
                .authentication_methods
                .into_iter()
                .take(MAX_EVENT_COLLECTION_ITEMS)
                .map(AuthenticationMethodDto::from)
                .collect(),
            sessions: value.sessions.into(),
            models: value.models.map(ModelCatalogDto::from),
            truncated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAgentDto {
    pub product: RuntimeAgentProductDto,
    pub version: Option<String>,
}

impl From<grok_runtime::RuntimeAgent> for RuntimeAgentDto {
    fn from(value: grok_runtime::RuntimeAgent) -> Self {
        Self {
            product: match value.product {
                grok_runtime::RuntimeAgentProduct::GrokBuild => RuntimeAgentProductDto::GrokBuild,
                grok_runtime::RuntimeAgentProduct::Other => RuntimeAgentProductDto::Other,
            },
            version: value
                .version
                .map(|version| bounded_text(version, MAX_DISPLAY_TEXT_BYTES).0),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAgentProductDto {
    GrokBuild,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationMethodDto {
    CachedToken,
    GrokCom,
    XaiApiKey,
    Grok,
    Other,
}

impl From<grok_runtime::AuthenticationMethod> for AuthenticationMethodDto {
    fn from(value: grok_runtime::AuthenticationMethod) -> Self {
        match value {
            grok_runtime::AuthenticationMethod::CachedToken => Self::CachedToken,
            grok_runtime::AuthenticationMethod::GrokCom => Self::GrokCom,
            grok_runtime::AuthenticationMethod::XaiApiKey => Self::XaiApiKey,
            grok_runtime::AuthenticationMethod::Grok => Self::Grok,
            grok_runtime::AuthenticationMethod::Other => Self::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionCapabilitiesDto {
    pub create: bool,
    pub prompt: bool,
    pub cancel: bool,
    pub list: bool,
    pub load: bool,
    pub resume: bool,
    pub close: bool,
}

impl From<grok_runtime::RuntimeSessionCapabilities> for RuntimeSessionCapabilitiesDto {
    fn from(value: grok_runtime::RuntimeSessionCapabilities) -> Self {
        Self {
            create: value.create,
            prompt: value.prompt,
            cancel: value.cancel,
            list: value.list,
            load: value.load,
            resume: value.resume,
            close: value.close,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogDto {
    pub current_model_id: String,
    pub available_models: Vec<ModelDto>,
    pub truncated: bool,
}

impl From<grok_runtime::ModelCatalog> for ModelCatalogDto {
    fn from(value: grok_runtime::ModelCatalog) -> Self {
        let truncated = value.available_models.len() > MAX_EVENT_COLLECTION_ITEMS;
        Self {
            current_model_id: value.current_model_id,
            available_models: value
                .available_models
                .into_iter()
                .take(MAX_EVENT_COLLECTION_ITEMS)
                .map(ModelDto::from)
                .collect(),
            truncated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDto {
    pub model_id: String,
    pub name: String,
    pub description: Option<String>,
    pub agent_type: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_efforts: Vec<ReasoningEffortDto>,
    pub supports_reasoning_effort: Option<bool>,
    pub total_context_tokens: Option<u64>,
    pub truncated: bool,
}

impl From<grok_runtime::ModelDescriptor> for ModelDto {
    fn from(value: grok_runtime::ModelDescriptor) -> Self {
        let truncated = value.reasoning_efforts.len() > MAX_EVENT_COLLECTION_ITEMS;
        Self {
            model_id: value.model_id,
            name: bounded_text(value.name, MAX_DISPLAY_TEXT_BYTES).0,
            description: value
                .description
                .map(|description| bounded_text(description, MAX_DISPLAY_TEXT_BYTES).0),
            agent_type: value.agent_type,
            reasoning_effort: value.reasoning_effort,
            reasoning_efforts: value
                .reasoning_efforts
                .into_iter()
                .take(MAX_EVENT_COLLECTION_ITEMS)
                .map(ReasoningEffortDto::from)
                .collect(),
            supports_reasoning_effort: value.supports_reasoning_effort,
            total_context_tokens: value.total_context_tokens,
            truncated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortDto {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub value: String,
    pub is_default: bool,
}

impl From<grok_runtime::ReasoningEffortOption> for ReasoningEffortDto {
    fn from(value: grok_runtime::ReasoningEffortOption) -> Self {
        Self {
            id: value.id,
            label: bounded_text(value.label, MAX_DISPLAY_TEXT_BYTES).0,
            description: value
                .description
                .map(|description| bounded_text(description, MAX_DISPLAY_TEXT_BYTES).0),
            value: value.value,
            is_default: value.is_default,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcknowledgementDto {
    pub acknowledged: bool,
}

impl AcknowledgementDto {
    pub const fn accepted() -> Self {
        Self { acknowledged: true }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResultDto {
    pub stop_reason: PromptStopReasonDto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptStopReasonDto {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Other,
}

impl From<grok_runtime::RuntimePromptStopReason> for PromptStopReasonDto {
    fn from(value: grok_runtime::RuntimePromptStopReason) -> Self {
        match value {
            grok_runtime::RuntimePromptStopReason::EndTurn => Self::EndTurn,
            grok_runtime::RuntimePromptStopReason::MaxTokens => Self::MaxTokens,
            grok_runtime::RuntimePromptStopReason::MaxTurnRequests => Self::MaxTurnRequests,
            grok_runtime::RuntimePromptStopReason::Refusal => Self::Refusal,
            grok_runtime::RuntimePromptStopReason::Cancelled => Self::Cancelled,
            grok_runtime::RuntimePromptStopReason::Other => Self::Other,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub session_id: String,
    pub models: Option<ModelCatalogDto>,
    pub legacy_config_options: Vec<LegacyConfigOptionDto>,
    pub controls: SessionControlsDto,
    pub truncated: bool,
}

impl From<grok_runtime::RuntimeSession> for SessionDto {
    fn from(value: grok_runtime::RuntimeSession) -> Self {
        let legacy = value.metadata.config_options.unwrap_or_default();
        let truncated = legacy.len() > MAX_EVENT_COLLECTION_ITEMS
            || value.controls.config_options.len() > MAX_EVENT_COLLECTION_ITEMS;
        Self {
            session_id: value.session_id,
            models: value.metadata.models.map(ModelCatalogDto::from),
            legacy_config_options: legacy
                .into_iter()
                .take(MAX_EVENT_COLLECTION_ITEMS)
                .map(LegacyConfigOptionDto::from)
                .collect(),
            controls: value.controls.into(),
            truncated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyConfigOptionDto {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub selected: Option<bool>,
}

impl From<grok_runtime::GrokSessionConfigOption> for LegacyConfigOptionDto {
    fn from(value: grok_runtime::GrokSessionConfigOption) -> Self {
        Self {
            id: value.id,
            label: bounded_text(value.label, MAX_DISPLAY_TEXT_BYTES).0,
            description: value
                .description
                .map(|description| bounded_text(description, MAX_DISPLAY_TEXT_BYTES).0),
            category: value.category,
            selected: value.selected,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionControlsDto {
    pub modes: Option<SessionModesDto>,
    pub config_options: Vec<ConfigOptionDto>,
    pub truncated: bool,
}

impl From<grok_runtime::RuntimeSessionControls> for SessionControlsDto {
    fn from(value: grok_runtime::RuntimeSessionControls) -> Self {
        let truncated = value.config_options.len() > MAX_EVENT_COLLECTION_ITEMS;
        Self {
            modes: value.modes.map(SessionModesDto::from),
            config_options: value
                .config_options
                .into_iter()
                .take(MAX_EVENT_COLLECTION_ITEMS)
                .map(ConfigOptionDto::from)
                .collect(),
            truncated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModesDto {
    pub current_mode_id: String,
    pub available_modes: Vec<SessionModeDto>,
    pub truncated: bool,
}

impl From<grok_runtime::RuntimeModes> for SessionModesDto {
    fn from(value: grok_runtime::RuntimeModes) -> Self {
        let truncated = value.available_modes.len() > MAX_EVENT_COLLECTION_ITEMS;
        Self {
            current_mode_id: value.current_mode_id,
            available_modes: value
                .available_modes
                .into_iter()
                .take(MAX_EVENT_COLLECTION_ITEMS)
                .map(SessionModeDto::from)
                .collect(),
            truncated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModeDto {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

impl From<grok_runtime::RuntimeMode> for SessionModeDto {
    fn from(value: grok_runtime::RuntimeMode) -> Self {
        Self {
            id: value.id,
            name: bounded_text(value.name, MAX_DISPLAY_TEXT_BYTES).0,
            description: value
                .description
                .map(|description| bounded_text(description, MAX_DISPLAY_TEXT_BYTES).0),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPageDto {
    pub sessions: Vec<SessionSummaryDto>,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

impl TryFrom<grok_runtime::RuntimeSessionPage> for SessionPageDto {
    type Error = ApplicationErrorDto;

    fn try_from(value: grok_runtime::RuntimeSessionPage) -> Result<Self, Self::Error> {
        let truncated = value.sessions.len() > MAX_EVENT_COLLECTION_ITEMS;
        let sessions = value
            .sessions
            .into_iter()
            .take(MAX_EVENT_COLLECTION_ITEMS)
            .map(SessionSummaryDto::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            sessions,
            next_cursor: value.next_cursor,
            truncated,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummaryDto {
    pub session_id: String,
    pub workspace: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

impl TryFrom<grok_runtime::RuntimeSessionSummary> for SessionSummaryDto {
    type Error = ApplicationErrorDto;

    fn try_from(value: grok_runtime::RuntimeSessionSummary) -> Result<Self, Self::Error> {
        let Some(workspace) = value.workspace.to_str() else {
            return Err(ApplicationErrorDto::boundary_violation());
        };
        if workspace.len() > MAX_WORKSPACE_PATH_BYTES {
            return Err(ApplicationErrorDto::boundary_violation());
        }
        Ok(Self {
            session_id: value.session_id,
            workspace: workspace.to_owned(),
            title: value
                .title
                .map(|title| bounded_text(title, MAX_DISPLAY_TEXT_BYTES).0),
            updated_at: value.updated_at,
        })
    }
}

pub fn session_from_response(
    response: grok_runtime::RuntimeResponse,
) -> Result<SessionDto, ApplicationErrorDto> {
    match response {
        grok_runtime::RuntimeResponse::Session(session) => Ok(session.into()),
        _ => Err(ApplicationErrorDto::unexpected_response()),
    }
}

pub fn sessions_from_response(
    response: grok_runtime::RuntimeResponse,
) -> Result<SessionPageDto, ApplicationErrorDto> {
    match response {
        grok_runtime::RuntimeResponse::Sessions(sessions) => sessions.try_into(),
        _ => Err(ApplicationErrorDto::unexpected_response()),
    }
}

pub fn prompt_from_response(
    response: grok_runtime::RuntimeResponse,
) -> Result<PromptResultDto, ApplicationErrorDto> {
    match response {
        grok_runtime::RuntimeResponse::PromptCompleted { stop_reason } => Ok(PromptResultDto {
            stop_reason: stop_reason.into(),
        }),
        _ => Err(ApplicationErrorDto::unexpected_response()),
    }
}

pub fn acknowledgement_from_response(
    response: grok_runtime::RuntimeResponse,
) -> Result<AcknowledgementDto, ApplicationErrorDto> {
    match response {
        grok_runtime::RuntimeResponse::Acknowledged => Ok(AcknowledgementDto::accepted()),
        _ => Err(ApplicationErrorDto::unexpected_response()),
    }
}

impl WorkspaceDto {
    #[cfg(test)]
    pub fn validate(request: WorkspaceRequestDto) -> Result<Self, ApplicationErrorDto> {
        let path = request.into_path()?;
        let canonical =
            crate::workspace::canonicalize_workspace(&path).map_err(ApplicationErrorDto::from)?;
        Self::from_canonical_path(canonical)
    }

    pub fn from_canonical_path(path: std::path::PathBuf) -> Result<Self, ApplicationErrorDto> {
        if !path.is_absolute() {
            return Err(ApplicationErrorDto::invalid_workspace());
        }
        let Some(path) = path.to_str() else {
            return Err(ApplicationErrorDto::boundary_violation());
        };
        if path.is_empty()
            || path.len() > MAX_WORKSPACE_PATH_BYTES
            || path.chars().any(char::is_control)
        {
            return Err(ApplicationErrorDto::invalid_workspace());
        }

        Ok(Self {
            path: path.to_owned(),
        })
    }
}

impl TryFrom<crate::workspace::RecentWorkspace> for RecentWorkspaceDto {
    type Error = ApplicationErrorDto;

    fn try_from(value: crate::workspace::RecentWorkspace) -> Result<Self, Self::Error> {
        let path = value
            .path
            .to_str()
            .filter(|path| {
                !path.is_empty()
                    && path.len() <= MAX_WORKSPACE_PATH_BYTES
                    && !path.chars().any(char::is_control)
            })
            .ok_or_else(ApplicationErrorDto::boundary_violation)?;
        Ok(Self {
            path: path.to_owned(),
            available: value.available,
            last_session_id: value.last_session_id.filter(|session_id| {
                !session_id.is_empty()
                    && session_id.len() <= MAX_IDENTIFIER_BYTES
                    && !session_id.chars().any(char::is_control)
            }),
        })
    }
}

impl RecentWorkspaceListDto {
    pub fn from_recent(
        workspaces: Vec<crate::workspace::RecentWorkspace>,
    ) -> Result<Self, ApplicationErrorDto> {
        Ok(Self {
            workspaces: workspaces
                .into_iter()
                .map(RecentWorkspaceDto::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationErrorDto {
    pub code: ApplicationErrorCodeDto,
    pub diagnostic: String,
    pub recoverable: bool,
}

impl ApplicationErrorDto {
    fn invalid_request() -> Self {
        Self {
            code: ApplicationErrorCodeDto::InvalidRequest,
            diagnostic: "request data was invalid or exceeded an application boundary".to_owned(),
            recoverable: false,
        }
    }

    pub fn invalid_external_url() -> Self {
        Self {
            code: ApplicationErrorCodeDto::InvalidRequest,
            diagnostic: "that link is not a safe http or https URL".to_owned(),
            recoverable: false,
        }
    }

    pub fn external_url_open_failed() -> Self {
        Self {
            code: ApplicationErrorCodeDto::ProtocolRequestFailed,
            diagnostic: "that link could not be opened".to_owned(),
            recoverable: true,
        }
    }

    fn invalid_workspace() -> Self {
        Self {
            code: ApplicationErrorCodeDto::InvalidWorkspace,
            diagnostic: "select an existing workspace directory".to_owned(),
            recoverable: true,
        }
    }

    #[cfg(not(windows))]
    pub fn workspace_picker_unavailable() -> Self {
        Self {
            code: ApplicationErrorCodeDto::CapabilityUnavailable,
            diagnostic: "native workspace selection is unavailable on this platform".to_owned(),
            recoverable: false,
        }
    }

    fn boundary_violation() -> Self {
        Self {
            code: ApplicationErrorCodeDto::BoundaryViolation,
            diagnostic: "application data could not cross the desktop boundary safely".to_owned(),
            recoverable: false,
        }
    }

    pub fn unexpected_response() -> Self {
        Self {
            code: ApplicationErrorCodeDto::UnexpectedResponse,
            diagnostic: "Grok returned an unexpected application response".to_owned(),
            recoverable: true,
        }
    }

    pub fn event_delivery_timeout() -> Self {
        Self {
            code: ApplicationErrorCodeDto::BoundaryViolation,
            diagnostic: "application event delivery did not reach a consistent checkpoint"
                .to_owned(),
            recoverable: true,
        }
    }
}

impl From<grok_runtime::RuntimeError> for ApplicationErrorDto {
    fn from(value: grok_runtime::RuntimeError) -> Self {
        Self {
            code: value.code.into(),
            diagnostic: bounded_text(value.diagnostic.into_inner(), MAX_DISPLAY_TEXT_BYTES).0,
            recoverable: value.recoverable,
        }
    }
}

impl From<crate::workspace::WorkspaceError> for ApplicationErrorDto {
    fn from(value: crate::workspace::WorkspaceError) -> Self {
        match value {
            crate::workspace::WorkspaceError::InvalidWorkspace => Self::invalid_workspace(),
            crate::workspace::WorkspaceError::PreferencesUnavailable => Self {
                code: ApplicationErrorCodeDto::PreferencesUnavailable,
                diagnostic: "recent workspaces could not be saved".to_owned(),
                recoverable: true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationErrorCodeDto {
    ExecutableUnavailable,
    ProcessStartFailed,
    ConnectionFailed,
    RequestTimedOut,
    InitializeFailed,
    UnsupportedProtocol,
    UnsupportedAuthentication,
    AuthenticationFailed,
    RuntimeStopped,
    InvalidWorkspace,
    InvalidRequest,
    CapabilityUnavailable,
    ProtocolRequestFailed,
    MalformedResponse,
    UnknownInteraction,
    DecisionUnavailable,
    UnexpectedResponse,
    BoundaryViolation,
    PreferencesUnavailable,
}

impl From<grok_runtime::RuntimeErrorCode> for ApplicationErrorCodeDto {
    fn from(value: grok_runtime::RuntimeErrorCode) -> Self {
        match value {
            grok_runtime::RuntimeErrorCode::ExecutableUnavailable => Self::ExecutableUnavailable,
            grok_runtime::RuntimeErrorCode::ProcessStartFailed => Self::ProcessStartFailed,
            grok_runtime::RuntimeErrorCode::ConnectionFailed => Self::ConnectionFailed,
            grok_runtime::RuntimeErrorCode::RequestTimedOut => Self::RequestTimedOut,
            grok_runtime::RuntimeErrorCode::InitializeFailed => Self::InitializeFailed,
            grok_runtime::RuntimeErrorCode::UnsupportedProtocol => Self::UnsupportedProtocol,
            grok_runtime::RuntimeErrorCode::UnsupportedAuthentication => {
                Self::UnsupportedAuthentication
            }
            grok_runtime::RuntimeErrorCode::AuthenticationFailed => Self::AuthenticationFailed,
            grok_runtime::RuntimeErrorCode::RuntimeStopped => Self::RuntimeStopped,
            grok_runtime::RuntimeErrorCode::InvalidWorkspace => Self::InvalidWorkspace,
            grok_runtime::RuntimeErrorCode::InvalidRequest => Self::InvalidRequest,
            grok_runtime::RuntimeErrorCode::CapabilityUnavailable => Self::CapabilityUnavailable,
            grok_runtime::RuntimeErrorCode::ProtocolRequestFailed => Self::ProtocolRequestFailed,
            grok_runtime::RuntimeErrorCode::MalformedResponse => Self::MalformedResponse,
            grok_runtime::RuntimeErrorCode::UnknownInteraction => Self::UnknownInteraction,
            grok_runtime::RuntimeErrorCode::DecisionUnavailable => Self::DecisionUnavailable,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationEventEnvelope {
    pub generation: u64,
    pub sequence: u64,
    pub event: ApplicationEvent,
}

pub struct ApplicationEventClock {
    state: Mutex<ApplicationEventClockState>,
    changed: tokio::sync::Notify,
}

struct ApplicationEventClockState {
    generation: u64,
    sequence: u64,
    runtime_state: ApplicationRuntimeState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApplicationEventCheckpoint {
    pub generation: u64,
    pub sequence: u64,
    pub runtime_state: ApplicationRuntimeState,
}

impl Default for ApplicationEventClock {
    fn default() -> Self {
        Self {
            state: Mutex::new(ApplicationEventClockState {
                generation: 0,
                sequence: 0,
                runtime_state: ApplicationRuntimeState::Disconnected,
            }),
            changed: tokio::sync::Notify::new(),
        }
    }
}

impl ApplicationEventClock {
    pub fn envelope(&self, event: ApplicationEvent) -> ApplicationEventEnvelope {
        let event = bounded_serialized_event(event);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.sequence = state
            .sequence
            .checked_add(1)
            .expect("application event sequence exhausted");
        ApplicationEventEnvelope {
            generation: state.generation,
            sequence: state.sequence,
            event,
        }
    }

    pub fn envelope_runtime(&self, event: grok_runtime::RuntimeEvent) -> ApplicationEventEnvelope {
        let event = bounded_serialized_event(ApplicationEvent::from_runtime(event));
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(
            &event,
            ApplicationEvent::RuntimeStateChanged {
                state: ApplicationRuntimeState::Connecting
            }
        ) {
            state.generation = state
                .generation
                .checked_add(1)
                .expect("application event generation exhausted");
            state.sequence = 0;
        }
        state.sequence = state
            .sequence
            .checked_add(1)
            .expect("application event sequence exhausted");
        if let ApplicationEvent::RuntimeStateChanged { state: next_state } = &event {
            state.runtime_state = *next_state;
        }
        let envelope = ApplicationEventEnvelope {
            generation: state.generation,
            sequence: state.sequence,
            event,
        };
        drop(state);
        self.changed.notify_waiters();
        envelope
    }

    pub fn checkpoint(&self) -> ApplicationEventCheckpoint {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ApplicationEventCheckpoint {
            generation: state.generation,
            sequence: state.sequence,
            runtime_state: state.runtime_state,
        }
    }

    pub async fn wait_for_runtime_state(
        &self,
        minimum_generation: u64,
        expected_state: ApplicationRuntimeState,
    ) -> ApplicationEventCheckpoint {
        loop {
            let changed = self.changed.notified();
            let checkpoint = self.checkpoint();
            if checkpoint.generation >= minimum_generation
                && checkpoint.runtime_state == expected_state
            {
                return checkpoint;
            }
            changed.await;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ApplicationEvent {
    RuntimeStateChanged {
        state: ApplicationRuntimeState,
    },
    SessionStateChanged {
        session_id: String,
        state: ApplicationSessionState,
    },
    UserMessageChunkReceived {
        session_id: String,
        message_id: Option<String>,
        text: String,
        truncated: bool,
    },
    MessageChunkReceived {
        session_id: String,
        message_id: Option<String>,
        text: String,
        truncated: bool,
    },
    ThoughtChunkReceived {
        session_id: String,
        thought_id: Option<String>,
        text: String,
        truncated: bool,
    },
    ToolCallChanged {
        session_id: String,
        call_id: String,
        title: String,
        kind: ToolCallKindDto,
        status: ActivityStatusDto,
        detail: Option<String>,
    },
    PermissionRequested {
        session_id: String,
        interaction_id: String,
        title: String,
        consequence: Option<String>,
        scope: PermissionScopeDto,
        available_decisions: Vec<PermissionDecisionDto>,
    },
    ElicitationRequested {
        session_id: Option<String>,
        interaction_id: String,
        prompt: String,
        control: ElicitationControlDto,
    },
    PlanChanged {
        session_id: String,
        entries: Vec<PlanEntryDto>,
        truncated: bool,
    },
    UsageChanged {
        session_id: String,
        usage: UsageDto,
    },
    SessionMetadataChanged {
        session_id: String,
    },
    AvailableCommandsChanged {
        session_id: String,
        commands: Vec<AvailableCommandDto>,
        truncated: bool,
    },
    SessionModeChanged {
        session_id: String,
        current_mode_id: String,
    },
    SessionConfigOptionsChanged {
        session_id: String,
        config_options: Vec<ConfigOptionDto>,
        truncated: bool,
    },
    SessionInfoChanged {
        session_id: String,
        title: OptionalUpdateDto,
        updated_at: OptionalUpdateDto,
    },
    RuntimeFailed {
        diagnostic: String,
        recoverable: bool,
    },
    RuntimeExtensionInvalidated {
        area: RuntimeExtensionAreaDto,
        session_id: Option<String>,
    },
    ExtensionObserved {
        classification: ObservedExtensionClassificationDto,
    },
    SessionActivated {
        session_id: String,
    },
    InteractionResolved {
        interaction_id: String,
        kind: InteractionKindDto,
    },
    InteractionsCleared {
        session_id: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKindDto {
    Permission,
    Elicitation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationRuntimeState {
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
pub enum ApplicationSessionState {
    Creating,
    Ready,
    Working,
    WaitingForInput,
    Cancelling,
    Cancelled,
    Completed,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallKindDto {
    Tool,
    TerminalCommand,
    BackgroundTask,
    Subagent,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatusDto {
    Pending,
    Running,
    WaitingForInput,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PermissionScopeDto {
    Tool {
        tool_name: String,
    },
    Command {
        command: String,
        working_directory: Option<String>,
        affected_paths: Vec<String>,
    },
    Filesystem {
        operation: String,
        path: String,
    },
    Network {
        destination: String,
    },
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ElicitationControlDto {
    Text {
        field_id: String,
        label: Option<String>,
        placeholder: Option<String>,
        sensitive: bool,
        min_length: usize,
        max_length: usize,
    },
    Confirmation {
        field_id: String,
        label: Option<String>,
    },
    Choice {
        field_id: String,
        label: Option<String>,
        options: Vec<String>,
        multiple: bool,
        truncated: bool,
    },
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntryDto {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: PlanEntryStatusDto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEntryStatusDto {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDto {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableCommandDto {
    pub name: String,
    pub description: String,
    pub accepts_input: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum OptionalUpdateDto {
    NotReported,
    Cleared,
    Value(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOptionDto {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: ConfigOptionKindDto,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ConfigOptionKindDto {
    Select {
        current_value: String,
        options: Vec<ConfigChoiceDto>,
        truncated: bool,
    },
    Boolean {
        current_value: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChoiceDto {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
    pub group: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeExtensionAreaDto {
    Models,
    Settings,
    Sessions,
    Queue,
    PromptCompletion,
    Session,
    Announcements,
    Mcp,
    Malformed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedExtensionClassificationDto {
    Unknown,
    Invalid,
}

impl ApplicationEvent {
    pub fn from_runtime(event: grok_runtime::RuntimeEvent) -> Self {
        match event {
            grok_runtime::RuntimeEvent::RuntimeStateChanged { state } => {
                Self::RuntimeStateChanged {
                    state: state.into(),
                }
            }
            grok_runtime::RuntimeEvent::SessionStateChanged { session_id, state } => {
                Self::SessionStateChanged {
                    session_id,
                    state: state.into(),
                }
            }
            grok_runtime::RuntimeEvent::SessionActivated { session_id } => {
                Self::SessionActivated { session_id }
            }
            grok_runtime::RuntimeEvent::UserMessageChunkReceived {
                session_id,
                message_id,
                text,
            } => {
                let (text, truncated) = bounded_text(text, MAX_EVENT_TEXT_BYTES);
                Self::UserMessageChunkReceived {
                    session_id,
                    message_id,
                    text,
                    truncated,
                }
            }
            grok_runtime::RuntimeEvent::MessageChunkReceived {
                session_id,
                message_id,
                text,
            } => {
                let (text, truncated) = bounded_text(text, MAX_EVENT_TEXT_BYTES);
                Self::MessageChunkReceived {
                    session_id,
                    message_id,
                    text,
                    truncated,
                }
            }
            grok_runtime::RuntimeEvent::ThoughtChunkReceived {
                session_id,
                thought_id,
                text,
            } => {
                let (text, truncated) = bounded_text(text, MAX_EVENT_TEXT_BYTES);
                Self::ThoughtChunkReceived {
                    session_id,
                    thought_id,
                    text,
                    truncated,
                }
            }
            grok_runtime::RuntimeEvent::ToolCallChanged {
                session_id,
                call_id,
                title,
                kind,
                status,
                detail,
            } => Self::ToolCallChanged {
                session_id,
                call_id,
                title: bounded_text(title, MAX_DISPLAY_TEXT_BYTES).0,
                kind: kind.into(),
                status: status.into(),
                detail: detail
                    .map(grok_runtime::RedactedDiagnostic::into_inner)
                    .map(|detail| bounded_text(detail, MAX_DISPLAY_TEXT_BYTES).0),
            },
            grok_runtime::RuntimeEvent::PermissionRequested {
                session_id,
                interaction_id,
                title,
                consequence,
                kind,
                available_decisions,
            } => {
                let Some(scope) = PermissionScopeDto::from_runtime(kind) else {
                    return Self::RuntimeFailed {
                        diagnostic: "permission request exceeded the safe application boundary"
                            .to_owned(),
                        recoverable: true,
                    };
                };
                Self::PermissionRequested {
                    session_id,
                    interaction_id,
                    title: bounded_text(title, MAX_DISPLAY_TEXT_BYTES).0,
                    consequence: consequence
                        .map(|value| bounded_text(value, MAX_DISPLAY_TEXT_BYTES).0),
                    scope,
                    available_decisions: available_decisions
                        .into_iter()
                        .take(5)
                        .map(PermissionDecisionDto::from)
                        .collect(),
                }
            }
            grok_runtime::RuntimeEvent::ElicitationRequested {
                session_id,
                interaction_id,
                prompt,
                kind,
            } => Self::ElicitationRequested {
                session_id,
                interaction_id,
                prompt: bounded_text(prompt, MAX_DISPLAY_TEXT_BYTES).0,
                control: kind.into(),
            },
            grok_runtime::RuntimeEvent::InteractionsCleared { session_id } => {
                Self::InteractionsCleared { session_id }
            }
            grok_runtime::RuntimeEvent::PlanChanged {
                session_id,
                entries,
            } => {
                let truncated = entries.len() > MAX_EVENT_COLLECTION_ITEMS;
                let entries = entries
                    .into_iter()
                    .take(MAX_EVENT_COLLECTION_ITEMS)
                    .map(PlanEntryDto::from)
                    .collect();
                Self::PlanChanged {
                    session_id,
                    entries,
                    truncated,
                }
            }
            grok_runtime::RuntimeEvent::UsageChanged { session_id, usage } => Self::UsageChanged {
                session_id,
                usage: usage.into(),
            },
            grok_runtime::RuntimeEvent::SessionMetadataChanged { session_id, .. } => {
                Self::SessionMetadataChanged { session_id }
            }
            grok_runtime::RuntimeEvent::AvailableCommandsChanged {
                session_id,
                commands,
            } => {
                let truncated = commands.len() > MAX_EVENT_COLLECTION_ITEMS;
                let commands = commands
                    .into_iter()
                    .take(MAX_EVENT_COLLECTION_ITEMS)
                    .map(AvailableCommandDto::from)
                    .collect();
                Self::AvailableCommandsChanged {
                    session_id,
                    commands,
                    truncated,
                }
            }
            grok_runtime::RuntimeEvent::SessionModeChanged {
                session_id,
                current_mode_id,
            } => Self::SessionModeChanged {
                session_id,
                current_mode_id,
            },
            grok_runtime::RuntimeEvent::SessionConfigOptionsChanged {
                session_id,
                config_options,
            } => {
                let truncated = config_options.len() > MAX_EVENT_COLLECTION_ITEMS;
                let config_options = config_options
                    .into_iter()
                    .take(MAX_EVENT_COLLECTION_ITEMS)
                    .map(ConfigOptionDto::from)
                    .collect();
                Self::SessionConfigOptionsChanged {
                    session_id,
                    config_options,
                    truncated,
                }
            }
            grok_runtime::RuntimeEvent::SessionInfoChanged {
                session_id,
                title,
                updated_at,
            } => Self::SessionInfoChanged {
                session_id,
                title: title.into(),
                updated_at: updated_at.into(),
            },
            grok_runtime::RuntimeEvent::RuntimeFailed {
                diagnostic,
                recoverable,
            } => Self::RuntimeFailed {
                diagnostic: bounded_text(diagnostic.into_inner(), MAX_DISPLAY_TEXT_BYTES).0,
                recoverable,
            },
            grok_runtime::RuntimeEvent::RuntimeExtensionChanged { update } => {
                let (area, session_id) = extension_invalidation(update);
                Self::RuntimeExtensionInvalidated { area, session_id }
            }
            grok_runtime::RuntimeEvent::ExtensionMethodObserved { method, .. } => {
                let classification = if method.as_str() == "x.ai/invalid_extension" {
                    ObservedExtensionClassificationDto::Invalid
                } else {
                    ObservedExtensionClassificationDto::Unknown
                };
                Self::ExtensionObserved { classification }
            }
        }
    }
}

impl From<grok_runtime::RuntimeState> for ApplicationRuntimeState {
    fn from(value: grok_runtime::RuntimeState) -> Self {
        match value {
            grok_runtime::RuntimeState::Connecting => Self::Connecting,
            grok_runtime::RuntimeState::Authenticating => Self::Authenticating,
            grok_runtime::RuntimeState::Ready => Self::Ready,
            grok_runtime::RuntimeState::Working => Self::Working,
            grok_runtime::RuntimeState::WaitingForInput => Self::WaitingForInput,
            grok_runtime::RuntimeState::Failed => Self::Failed,
            grok_runtime::RuntimeState::Disconnected => Self::Disconnected,
        }
    }
}

impl From<grok_runtime::SessionState> for ApplicationSessionState {
    fn from(value: grok_runtime::SessionState) -> Self {
        match value {
            grok_runtime::SessionState::Creating => Self::Creating,
            grok_runtime::SessionState::Ready => Self::Ready,
            grok_runtime::SessionState::Working => Self::Working,
            grok_runtime::SessionState::WaitingForInput => Self::WaitingForInput,
            grok_runtime::SessionState::Cancelling => Self::Cancelling,
            grok_runtime::SessionState::Cancelled => Self::Cancelled,
            grok_runtime::SessionState::Completed => Self::Completed,
            grok_runtime::SessionState::Closed => Self::Closed,
            grok_runtime::SessionState::Failed => Self::Failed,
        }
    }
}

impl From<grok_runtime::ToolCallKind> for ToolCallKindDto {
    fn from(value: grok_runtime::ToolCallKind) -> Self {
        match value {
            grok_runtime::ToolCallKind::Tool => Self::Tool,
            grok_runtime::ToolCallKind::TerminalCommand => Self::TerminalCommand,
            grok_runtime::ToolCallKind::BackgroundTask => Self::BackgroundTask,
            grok_runtime::ToolCallKind::Subagent => Self::Subagent,
            grok_runtime::ToolCallKind::Other => Self::Other,
        }
    }
}

impl From<grok_runtime::ActivityStatus> for ActivityStatusDto {
    fn from(value: grok_runtime::ActivityStatus) -> Self {
        match value {
            grok_runtime::ActivityStatus::Pending => Self::Pending,
            grok_runtime::ActivityStatus::Running => Self::Running,
            grok_runtime::ActivityStatus::WaitingForInput => Self::WaitingForInput,
            grok_runtime::ActivityStatus::Completed => Self::Completed,
            grok_runtime::ActivityStatus::Failed => Self::Failed,
            grok_runtime::ActivityStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl PermissionScopeDto {
    fn from_runtime(value: grok_runtime::PermissionKind) -> Option<Self> {
        match value {
            grok_runtime::PermissionKind::Tool { tool_name } => {
                validate_output_text(&tool_name, MAX_DISPLAY_TEXT_BYTES)?;
                Some(Self::Tool { tool_name })
            }
            grok_runtime::PermissionKind::Command {
                command,
                working_directory,
                affected_paths,
            } => {
                validate_output_text(&command, MAX_DISPLAY_TEXT_BYTES)?;
                let working_directory = match working_directory {
                    Some(path) => {
                        let value = path.to_str()?.to_owned();
                        validate_output_text(&value, MAX_WORKSPACE_PATH_BYTES)?;
                        Some(value)
                    }
                    None => None,
                };
                Some(Self::Command {
                    command,
                    working_directory,
                    affected_paths: {
                        if affected_paths.len() > 64 {
                            return None;
                        }
                        affected_paths
                            .into_iter()
                            .map(|path| {
                                let value = path.to_str()?.to_owned();
                                validate_output_text(&value, MAX_WORKSPACE_PATH_BYTES)?;
                                Some(value)
                            })
                            .collect::<Option<Vec<_>>>()?
                    },
                })
            }
            grok_runtime::PermissionKind::Filesystem { operation, path } => {
                validate_output_text(&operation, MAX_DISPLAY_TEXT_BYTES)?;
                let path = path.to_str()?.to_owned();
                validate_output_text(&path, MAX_WORKSPACE_PATH_BYTES)?;
                Some(Self::Filesystem { operation, path })
            }
            grok_runtime::PermissionKind::Network { destination } => {
                validate_output_text(&destination, MAX_DISPLAY_TEXT_BYTES)?;
                Some(Self::Network { destination })
            }
            grok_runtime::PermissionKind::Other => Some(Self::Other),
        }
    }
}

impl From<grok_runtime::ElicitationKind> for ElicitationControlDto {
    fn from(value: grok_runtime::ElicitationKind) -> Self {
        match value {
            grok_runtime::ElicitationKind::Text {
                field_id,
                label,
                placeholder,
                sensitive,
                min_length,
                max_length,
            } => Self::Text {
                field_id,
                label: label.map(|value| bounded_text(value, MAX_DISPLAY_TEXT_BYTES).0),
                placeholder: placeholder.map(|value| bounded_text(value, MAX_DISPLAY_TEXT_BYTES).0),
                sensitive,
                min_length,
                max_length,
            },
            grok_runtime::ElicitationKind::Confirmation { field_id, label } => Self::Confirmation {
                field_id,
                label: label.map(|value| bounded_text(value, MAX_DISPLAY_TEXT_BYTES).0),
            },
            grok_runtime::ElicitationKind::Choice {
                field_id,
                label,
                options,
                multiple,
            } => {
                let truncated = options.len() > MAX_EVENT_COLLECTION_ITEMS;
                Self::Choice {
                    field_id,
                    label: label.map(|value| bounded_text(value, MAX_DISPLAY_TEXT_BYTES).0),
                    options: options
                        .into_iter()
                        .take(MAX_EVENT_COLLECTION_ITEMS)
                        .map(|value| bounded_text(value, MAX_DISPLAY_TEXT_BYTES).0)
                        .collect(),
                    multiple,
                    truncated,
                }
            }
            grok_runtime::ElicitationKind::Other => Self::Other,
        }
    }
}

impl From<grok_runtime::PlanEntry> for PlanEntryDto {
    fn from(value: grok_runtime::PlanEntry) -> Self {
        Self {
            id: value.id,
            title: bounded_text(value.title, MAX_DISPLAY_TEXT_BYTES).0,
            description: value
                .description
                .map(|value| bounded_text(value, MAX_DISPLAY_TEXT_BYTES).0),
            status: match value.status {
                grok_runtime::PlanEntryStatus::Pending => PlanEntryStatusDto::Pending,
                grok_runtime::PlanEntryStatus::InProgress => PlanEntryStatusDto::InProgress,
                grok_runtime::PlanEntryStatus::Completed => PlanEntryStatusDto::Completed,
            },
        }
    }
}

impl From<grok_runtime::Usage> for UsageDto {
    fn from(value: grok_runtime::Usage) -> Self {
        Self {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cached_input_tokens: value.cached_input_tokens,
            total_tokens: value.total_tokens,
            context_window_tokens: value.context_window_tokens,
        }
    }
}

impl From<grok_runtime::RuntimeAvailableCommand> for AvailableCommandDto {
    fn from(value: grok_runtime::RuntimeAvailableCommand) -> Self {
        Self {
            name: value.name,
            description: bounded_text(value.description, MAX_DISPLAY_TEXT_BYTES).0,
            accepts_input: value.accepts_input,
        }
    }
}

impl From<grok_runtime::RuntimeOptionalUpdate<String>> for OptionalUpdateDto {
    fn from(value: grok_runtime::RuntimeOptionalUpdate<String>) -> Self {
        match value {
            grok_runtime::RuntimeOptionalUpdate::NotReported => Self::NotReported,
            grok_runtime::RuntimeOptionalUpdate::Cleared => Self::Cleared,
            grok_runtime::RuntimeOptionalUpdate::Value(value) => {
                Self::Value(bounded_text(value, MAX_DISPLAY_TEXT_BYTES).0)
            }
        }
    }
}

impl From<grok_runtime::RuntimeConfigOption> for ConfigOptionDto {
    fn from(value: grok_runtime::RuntimeConfigOption) -> Self {
        Self {
            id: value.id,
            name: bounded_text(value.name, MAX_DISPLAY_TEXT_BYTES).0,
            description: value
                .description
                .map(|description| bounded_text(description, MAX_DISPLAY_TEXT_BYTES).0),
            kind: value.kind.into(),
        }
    }
}

impl From<grok_runtime::RuntimeConfigKind> for ConfigOptionKindDto {
    fn from(value: grok_runtime::RuntimeConfigKind) -> Self {
        match value {
            grok_runtime::RuntimeConfigKind::Select {
                current_value,
                options,
            } => {
                let truncated = options.len() > MAX_EVENT_COLLECTION_ITEMS;
                Self::Select {
                    current_value,
                    options: options
                        .into_iter()
                        .take(MAX_EVENT_COLLECTION_ITEMS)
                        .map(ConfigChoiceDto::from)
                        .collect(),
                    truncated,
                }
            }
            grok_runtime::RuntimeConfigKind::Boolean { current_value } => {
                Self::Boolean { current_value }
            }
        }
    }
}

impl From<grok_runtime::RuntimeConfigChoice> for ConfigChoiceDto {
    fn from(value: grok_runtime::RuntimeConfigChoice) -> Self {
        Self {
            value: value.value,
            name: bounded_text(value.name, MAX_DISPLAY_TEXT_BYTES).0,
            description: value
                .description
                .map(|description| bounded_text(description, MAX_DISPLAY_TEXT_BYTES).0),
            group: value.group,
        }
    }
}

fn extension_invalidation(
    update: grok_runtime::RuntimeExtensionUpdate,
) -> (RuntimeExtensionAreaDto, Option<String>) {
    match update {
        grok_runtime::RuntimeExtensionUpdate::Models(_) => (RuntimeExtensionAreaDto::Models, None),
        grok_runtime::RuntimeExtensionUpdate::Settings(_) => {
            (RuntimeExtensionAreaDto::Settings, None)
        }
        grok_runtime::RuntimeExtensionUpdate::Sessions(_) => {
            (RuntimeExtensionAreaDto::Sessions, None)
        }
        grok_runtime::RuntimeExtensionUpdate::Queue(queue) => {
            (RuntimeExtensionAreaDto::Queue, Some(queue.session_id))
        }
        grok_runtime::RuntimeExtensionUpdate::PromptCompletion(completion) => (
            RuntimeExtensionAreaDto::PromptCompletion,
            Some(completion.session_id),
        ),
        grok_runtime::RuntimeExtensionUpdate::Session(session) => (
            RuntimeExtensionAreaDto::Session,
            Some(session.session_id.clone()),
        ),
        grok_runtime::RuntimeExtensionUpdate::Announcements(_) => {
            (RuntimeExtensionAreaDto::Announcements, None)
        }
        grok_runtime::RuntimeExtensionUpdate::Mcp(update) => {
            let session_id = match update {
                grok_runtime::McpUpdate::InitProgress { session_id, .. } => session_id,
                grok_runtime::McpUpdate::ServerStatus { session_id, .. }
                | grok_runtime::McpUpdate::Initialized { session_id, .. } => Some(session_id),
                grok_runtime::McpUpdate::ServersUpdated { .. } => None,
            };
            (RuntimeExtensionAreaDto::Mcp, session_id)
        }
        grok_runtime::RuntimeExtensionUpdate::Malformed(_) => {
            (RuntimeExtensionAreaDto::Malformed, None)
        }
    }
}

fn bounded_text(value: String, maximum_bytes: usize) -> (String, bool) {
    if value.len() <= maximum_bytes {
        return (value, false);
    }

    let prefix_budget = maximum_bytes.saturating_sub(TRUNCATED_SUFFIX.len());
    let mut end = prefix_budget;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = value[..end].to_owned();
    bounded.push_str(TRUNCATED_SUFFIX);
    (bounded, true)
}

fn validate_token(value: &str, maximum_bytes: usize) -> Result<(), ApplicationErrorDto> {
    if value.is_empty() || value.len() > maximum_bytes || value.chars().any(char::is_control) {
        Err(ApplicationErrorDto::invalid_request())
    } else {
        Ok(())
    }
}

pub fn validate_external_url(value: &str) -> Result<String, ApplicationErrorDto> {
    if value.is_empty()
        || value.len() > MAX_EXTERNAL_URL_BYTES
        || value.chars().any(|character| {
            character.is_whitespace() || character.is_control() || character == '\\'
        })
    {
        return Err(ApplicationErrorDto::invalid_external_url());
    }
    let rest = split_http_url(value).ok_or_else(ApplicationErrorDto::invalid_external_url)?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return Err(ApplicationErrorDto::invalid_external_url());
    }
    let host = authority.split(':').next().unwrap_or("");
    if host.is_empty() {
        return Err(ApplicationErrorDto::invalid_external_url());
    }
    Ok(value.to_owned())
}

fn split_http_url(value: &str) -> Option<&str> {
    for prefix in ["https://", "http://"] {
        if value.len() >= prefix.len()
            && value.is_char_boundary(prefix.len())
            && value[..prefix.len()].eq_ignore_ascii_case(prefix)
        {
            return Some(&value[prefix.len()..]);
        }
    }
    None
}

fn validate_workspace_input(value: &str) -> Result<(), ApplicationErrorDto> {
    if value.is_empty()
        || value.len() > MAX_WORKSPACE_PATH_BYTES
        || value.chars().any(char::is_control)
    {
        Err(ApplicationErrorDto::invalid_request())
    } else {
        Ok(())
    }
}

fn validate_elicitation_text(value: &str) -> Result<(), ApplicationErrorDto> {
    if value.len() > MAX_ELICITATION_VALUE_BYTES || value.chars().any(char::is_control) {
        Err(ApplicationErrorDto::invalid_request())
    } else {
        Ok(())
    }
}

fn validate_output_text(value: &str, maximum_bytes: usize) -> Option<()> {
    (!value.is_empty() && value.len() <= maximum_bytes && !value.chars().any(char::is_control))
        .then_some(())
}

fn bounded_serialized_event(event: ApplicationEvent) -> ApplicationEvent {
    match serde_json::to_vec(&event) {
        Ok(encoded) if encoded.len() <= MAX_SERIALIZED_EVENT_BYTES => event,
        _ => ApplicationEvent::RuntimeFailed {
            diagnostic: "runtime event exceeded the safe application boundary".to_owned(),
            recoverable: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn application_event_contract_fixtures() -> Vec<ApplicationEvent> {
        let session_id = "session-1".to_owned();

        vec![
            ApplicationEvent::RuntimeStateChanged {
                state: ApplicationRuntimeState::Ready,
            },
            ApplicationEvent::SessionStateChanged {
                session_id: session_id.clone(),
                state: ApplicationSessionState::Working,
            },
            ApplicationEvent::UserMessageChunkReceived {
                session_id: session_id.clone(),
                message_id: Some("user-1".to_owned()),
                text: "hello".to_owned(),
                truncated: false,
            },
            ApplicationEvent::MessageChunkReceived {
                session_id: session_id.clone(),
                message_id: Some("assistant-1".to_owned()),
                text: "hi".to_owned(),
                truncated: false,
            },
            ApplicationEvent::ThoughtChunkReceived {
                session_id: session_id.clone(),
                thought_id: None,
                text: "thinking".to_owned(),
                truncated: false,
            },
            ApplicationEvent::ToolCallChanged {
                session_id: session_id.clone(),
                call_id: "call-1".to_owned(),
                title: "Inspect workspace".to_owned(),
                kind: ToolCallKindDto::Tool,
                status: ActivityStatusDto::Running,
                detail: Some("bounded detail".to_owned()),
            },
            ApplicationEvent::PermissionRequested {
                session_id: session_id.clone(),
                interaction_id: "permission-1".to_owned(),
                title: "Run tests".to_owned(),
                consequence: Some("Creates build artifacts".to_owned()),
                scope: PermissionScopeDto::Command {
                    command: "cargo test".to_owned(),
                    working_directory: Some("C:\\workspace".to_owned()),
                    affected_paths: vec![],
                },
                available_decisions: vec![
                    PermissionDecisionDto::AllowOnce,
                    PermissionDecisionDto::DenyOnce,
                ],
            },
            ApplicationEvent::ElicitationRequested {
                session_id: Some(session_id.clone()),
                interaction_id: "elicitation-1".to_owned(),
                prompt: "Choose".to_owned(),
                control: ElicitationControlDto::Choice {
                    field_id: "answer".to_owned(),
                    label: Some("Answer".to_owned()),
                    options: vec!["A".to_owned(), "B".to_owned()],
                    multiple: false,
                    truncated: false,
                },
            },
            ApplicationEvent::PlanChanged {
                session_id: session_id.clone(),
                entries: vec![PlanEntryDto {
                    id: "step-1".to_owned(),
                    title: "Probe runtime".to_owned(),
                    description: None,
                    status: PlanEntryStatusDto::InProgress,
                }],
                truncated: false,
            },
            ApplicationEvent::UsageChanged {
                session_id: session_id.clone(),
                usage: UsageDto {
                    total_tokens: Some(42),
                    ..UsageDto::default()
                },
            },
            ApplicationEvent::SessionMetadataChanged {
                session_id: session_id.clone(),
            },
            ApplicationEvent::AvailableCommandsChanged {
                session_id: session_id.clone(),
                commands: vec![AvailableCommandDto {
                    name: "review".to_owned(),
                    description: "Review changes".to_owned(),
                    accepts_input: false,
                }],
                truncated: false,
            },
            ApplicationEvent::SessionModeChanged {
                session_id: session_id.clone(),
                current_mode_id: "plan".to_owned(),
            },
            ApplicationEvent::SessionConfigOptionsChanged {
                session_id: session_id.clone(),
                config_options: vec![ConfigOptionDto {
                    id: "effort".to_owned(),
                    name: "Reasoning effort".to_owned(),
                    description: None,
                    kind: ConfigOptionKindDto::Select {
                        current_value: "high".to_owned(),
                        options: vec![ConfigChoiceDto {
                            value: "high".to_owned(),
                            name: "High".to_owned(),
                            description: None,
                            group: None,
                        }],
                        truncated: false,
                    },
                }],
                truncated: false,
            },
            ApplicationEvent::SessionInfoChanged {
                session_id,
                title: OptionalUpdateDto::Value("Fixture".to_owned()),
                updated_at: OptionalUpdateDto::NotReported,
            },
            ApplicationEvent::RuntimeFailed {
                diagnostic: "runtime failed safely".to_owned(),
                recoverable: true,
            },
            ApplicationEvent::RuntimeExtensionInvalidated {
                area: RuntimeExtensionAreaDto::Models,
                session_id: None,
            },
            ApplicationEvent::ExtensionObserved {
                classification: ObservedExtensionClassificationDto::Unknown,
            },
            ApplicationEvent::SessionActivated {
                session_id: "session-1".to_owned(),
            },
            ApplicationEvent::InteractionResolved {
                interaction_id: "permission-1".to_owned(),
                kind: InteractionKindDto::Permission,
            },
            ApplicationEvent::InteractionsCleared {
                session_id: Some("session-1".to_owned()),
            },
        ]
    }

    #[test]
    fn every_application_event_variant_round_trips_through_json() {
        for event in application_event_contract_fixtures() {
            let encoded = serde_json::to_string(&event).expect("event should serialize");
            let decoded: ApplicationEvent =
                serde_json::from_str(&encoded).expect("event should deserialize");

            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn rust_wire_shapes_match_the_shared_application_contract_manifest() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../fixtures/application-contract-manifest.json"
        ))
        .expect("contract manifest should be valid JSON");
        assert_eq!(manifest["version"], 1);
        assert_eq!(manifest["eventName"], APPLICATION_EVENT_NAME);
        assert_eq!(
            manifest["commands"],
            serde_json::to_value(APPLICATION_COMMAND_NAMES).expect("commands should serialize")
        );

        let expected_events = manifest["events"]
            .as_object()
            .expect("manifest events should be an object");
        let mut observed = std::collections::BTreeSet::new();
        for event in application_event_contract_fixtures() {
            let value = serde_json::to_value(event).expect("event should serialize");
            let object = value
                .as_object()
                .expect("event should serialize as an object");
            let event_type = object["type"]
                .as_str()
                .expect("event should carry its type tag");
            assert!(observed.insert(event_type.to_owned()), "duplicate fixture");

            let mut actual_fields = object
                .keys()
                .filter(|field| field.as_str() != "type")
                .cloned()
                .collect::<Vec<_>>();
            actual_fields.sort();
            assert_eq!(
                serde_json::to_value(actual_fields).expect("fields should serialize"),
                expected_events[event_type],
                "wire fields drifted for {event_type}"
            );
        }
        assert_eq!(observed.len(), expected_events.len());
    }

    #[test]
    fn rust_dto_fields_and_enum_values_match_the_shared_manifest() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../fixtures/application-contract-manifest.json"
        ))
        .expect("contract manifest should be valid JSON");
        let dto_fields = manifest["dtoFields"]
            .as_object()
            .expect("DTO fields should be an object");
        let enum_values = manifest["enumValues"]
            .as_object()
            .expect("enum values should be an object");
        let variant_fields = manifest["variantFields"]
            .as_object()
            .expect("variant fields should be an object");

        macro_rules! assert_fields {
            ($name:literal, $dto:ty, $fixture:expr) => {{
                let dto: $dto = serde_json::from_value($fixture)
                    .unwrap_or_else(|error| panic!("{} fixture failed: {error}", $name));
                let value = serde_json::to_value(dto).expect("DTO should serialize");
                let mut fields = value
                    .as_object()
                    .expect("DTO should be an object")
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>();
                fields.sort();
                assert_eq!(
                    serde_json::to_value(fields).expect("fields should serialize"),
                    dto_fields[$name],
                    "DTO fields drifted for {}",
                    $name
                );
            }};
        }
        macro_rules! assert_values {
            ($name:literal, [$($value:expr),+ $(,)?]) => {
                assert_eq!(
                    serde_json::to_value([$($value),+]).expect("enum values should serialize"),
                    enum_values[$name],
                    "enum values drifted for {}",
                    $name
                );
            };
        }
        macro_rules! assert_variant_fields {
            ($group:literal, $tag:literal, [$($value:expr),+ $(,)?]) => {{
                let expected = variant_fields[$group]
                    .as_object()
                    .expect("variant group should be an object");
                let mut observed = std::collections::BTreeSet::new();
                for value in [$($value),+] {
                    let value = serde_json::to_value(value).expect("variant should serialize");
                    let object = value.as_object().expect("variant should be an object");
                    let variant = object[$tag].as_str().expect("variant tag should be text");
                    assert!(observed.insert(variant.to_owned()), "duplicate variant fixture");
                    let mut fields = object
                        .keys()
                        .filter(|field| field.as_str() != $tag)
                        .cloned()
                        .collect::<Vec<_>>();
                    fields.sort();
                    assert_eq!(
                        serde_json::to_value(fields).expect("fields should serialize"),
                        expected[variant],
                        "variant fields drifted for {}.{}",
                        $group,
                        variant
                    );
                }
                assert_eq!(observed.len(), expected.len(), "variant fixture coverage drifted");
            }};
        }

        use serde_json::json;
        assert_fields!(
            "applicationEventEnvelope",
            ApplicationEventEnvelope,
            json!({
                "generation": 1, "sequence": 1,
                "event": { "type": "runtime_state_changed", "state": "ready" }
            })
        );
        assert_fields!(
            "applicationError",
            ApplicationErrorDto,
            json!({
                "code": "invalid_request", "diagnostic": "safe", "recoverable": false
            })
        );
        assert_fields!(
            "workspaceRequest",
            WorkspaceRequestDto,
            json!({ "path": "C:\\work" })
        );
        assert_fields!(
            "openExternalUrlRequest",
            OpenExternalUrlRequestDto,
            json!({ "url": "https://example.com/docs" })
        );
        assert_fields!("workspace", WorkspaceDto, json!({ "path": "C:\\work" }));
        assert_fields!(
            "recentWorkspace",
            RecentWorkspaceDto,
            json!({ "path": "C:\\work", "available": true, "lastSessionId": null })
        );
        assert_fields!(
            "recentWorkspaceList",
            RecentWorkspaceListDto,
            json!({ "workspaces": [] })
        );
        assert_fields!(
            "newSessionRequest",
            NewSessionRequestDto,
            json!({ "workspace": "C:\\work" })
        );
        assert_fields!(
            "listSessionsRequest",
            ListSessionsRequestDto,
            json!({
                "workspace": null, "cursor": null
            })
        );
        assert_fields!(
            "sessionWorkspaceRequest",
            SessionWorkspaceRequestDto,
            json!({
                "sessionId": "session-1", "workspace": "C:\\work"
            })
        );
        assert_fields!(
            "sessionRequest",
            SessionRequestDto,
            json!({ "sessionId": "session-1" })
        );
        assert_fields!(
            "promptRequest",
            PromptRequestDto,
            json!({
                "sessionId": "session-1", "text": "hello"
            })
        );
        assert_fields!(
            "setSessionModeRequest",
            SetSessionModeRequestDto,
            json!({
                "sessionId": "session-1", "modeId": "plan"
            })
        );
        assert_fields!(
            "setSessionModelRequest",
            SetSessionModelRequestDto,
            json!({
                "sessionId": "session-1", "modelId": "model-1", "reasoningEffort": null
            })
        );
        assert_fields!(
            "setSessionConfigRequest",
            SetSessionConfigRequestDto,
            json!({
                "sessionId": "session-1", "configId": "effort",
                "value": { "type": "boolean", "value": true }
            })
        );
        assert_fields!(
            "permissionResponseRequest",
            PermissionResponseRequestDto,
            json!({
                "interactionId": "permission-1", "decision": "deny_once"
            })
        );
        assert_fields!(
            "elicitationResponseRequest",
            ElicitationResponseRequestDto,
            json!({
                "interactionId": "elicitation-1", "decision": { "type": "decline" }
            })
        );
        assert_fields!(
            "setupStatus",
            SetupStatusDto,
            json!({
                "runtimeAvailable": false,
                "executableState": "missing",
                "executableSource": null,
                "failure": { "code": "executable_unavailable", "diagnostic": "missing", "recoverable": true }
            })
        );
        assert_fields!(
            "acknowledgement",
            AcknowledgementDto,
            json!({ "acknowledged": true })
        );
        assert_fields!(
            "promptResult",
            PromptResultDto,
            json!({ "stopReason": "end_turn" })
        );
        assert_fields!(
            "runtimeSnapshot",
            RuntimeSnapshotDto,
            json!({
                "generation": 1, "lastSequence": 2, "state": "ready", "capabilities": null
            })
        );
        assert_fields!(
            "runtimeCapabilities",
            RuntimeCapabilitiesDto,
            json!({
                "protocolVersion": 1,
                "agent": { "product": "grok_build", "version": null },
                "authenticationMethods": ["cached_token"],
                "sessions": { "create": true, "prompt": true, "cancel": true, "list": true, "load": true, "resume": true, "close": true },
                "models": null, "truncated": false
            })
        );
        assert_fields!(
            "runtimeAgent",
            RuntimeAgentDto,
            json!({
                "product": "grok_build", "version": null
            })
        );
        assert_fields!(
            "runtimeSessionCapabilities",
            RuntimeSessionCapabilitiesDto,
            json!({
                "create": true, "prompt": true, "cancel": true, "list": true,
                "load": true, "resume": true, "close": true
            })
        );
        assert_fields!(
            "modelCatalog",
            ModelCatalogDto,
            json!({
                "currentModelId": "model-1", "availableModels": [], "truncated": false
            })
        );
        assert_fields!(
            "model",
            ModelDto,
            json!({
                "modelId": "model-1", "name": "Model", "description": null, "agentType": null,
                "reasoningEffort": null, "reasoningEfforts": [], "supportsReasoningEffort": null,
                "totalContextTokens": null, "truncated": false
            })
        );
        assert_fields!(
            "reasoningEffort",
            ReasoningEffortDto,
            json!({
                "id": "high", "label": "High", "description": null,
                "value": "high", "isDefault": true
            })
        );
        assert_fields!(
            "session",
            SessionDto,
            json!({
                "sessionId": "session-1", "models": null, "legacyConfigOptions": [],
                "controls": { "modes": null, "configOptions": [], "truncated": false },
                "truncated": false
            })
        );
        assert_fields!(
            "legacyConfigOption",
            LegacyConfigOptionDto,
            json!({
                "id": "legacy", "label": "Legacy", "description": null,
                "category": null, "selected": null
            })
        );
        assert_fields!(
            "sessionControls",
            SessionControlsDto,
            json!({
                "modes": null, "configOptions": [], "truncated": false
            })
        );
        assert_fields!(
            "sessionModes",
            SessionModesDto,
            json!({
                "currentModeId": "plan", "availableModes": [], "truncated": false
            })
        );
        assert_fields!(
            "sessionMode",
            SessionModeDto,
            json!({
                "id": "plan", "name": "Plan", "description": null
            })
        );
        assert_fields!(
            "sessionPage",
            SessionPageDto,
            json!({
                "sessions": [], "nextCursor": null, "truncated": false
            })
        );
        assert_fields!(
            "sessionSummary",
            SessionSummaryDto,
            json!({
                "sessionId": "session-1", "workspace": "C:\\work", "title": null, "updatedAt": null
            })
        );
        assert_fields!(
            "planEntry",
            PlanEntryDto,
            json!({
                "id": "step-1", "title": "Step", "description": null, "status": "pending"
            })
        );
        assert_fields!(
            "usage",
            UsageDto,
            json!({
                "inputTokens": null, "outputTokens": null, "cachedInputTokens": null,
                "totalTokens": 10, "contextWindowTokens": null
            })
        );
        assert_fields!(
            "availableCommand",
            AvailableCommandDto,
            json!({
                "name": "review", "description": "Review", "acceptsInput": false
            })
        );
        assert_fields!(
            "configOption",
            ConfigOptionDto,
            json!({
                "id": "effort", "name": "Effort", "description": null,
                "kind": { "type": "boolean", "currentValue": true }
            })
        );
        assert_fields!(
            "configChoice",
            ConfigChoiceDto,
            json!({
                "value": "high", "name": "High", "description": null, "group": null
            })
        );
        assert_eq!(dto_fields.len(), 39, "every reviewed DTO needs a fixture");

        assert_values!(
            "runtimeState",
            [
                ApplicationRuntimeState::Connecting,
                ApplicationRuntimeState::Authenticating,
                ApplicationRuntimeState::Ready,
                ApplicationRuntimeState::Working,
                ApplicationRuntimeState::WaitingForInput,
                ApplicationRuntimeState::Failed,
                ApplicationRuntimeState::Disconnected
            ]
        );
        assert_values!(
            "sessionState",
            [
                ApplicationSessionState::Creating,
                ApplicationSessionState::Ready,
                ApplicationSessionState::Working,
                ApplicationSessionState::WaitingForInput,
                ApplicationSessionState::Cancelling,
                ApplicationSessionState::Cancelled,
                ApplicationSessionState::Completed,
                ApplicationSessionState::Closed,
                ApplicationSessionState::Failed
            ]
        );
        assert_values!(
            "activityStatus",
            [
                ActivityStatusDto::Pending,
                ActivityStatusDto::Running,
                ActivityStatusDto::WaitingForInput,
                ActivityStatusDto::Completed,
                ActivityStatusDto::Failed,
                ActivityStatusDto::Cancelled
            ]
        );
        assert_values!(
            "toolCallKind",
            [
                ToolCallKindDto::Tool,
                ToolCallKindDto::TerminalCommand,
                ToolCallKindDto::BackgroundTask,
                ToolCallKindDto::Subagent,
                ToolCallKindDto::Other
            ]
        );
        assert_values!(
            "permissionDecision",
            [
                PermissionDecisionDto::AllowOnce,
                PermissionDecisionDto::AllowAlways,
                PermissionDecisionDto::DenyOnce,
                PermissionDecisionDto::DenyAlways,
                PermissionDecisionDto::Cancel
            ]
        );
        assert_values!(
            "runtimeExtensionArea",
            [
                RuntimeExtensionAreaDto::Models,
                RuntimeExtensionAreaDto::Settings,
                RuntimeExtensionAreaDto::Sessions,
                RuntimeExtensionAreaDto::Queue,
                RuntimeExtensionAreaDto::PromptCompletion,
                RuntimeExtensionAreaDto::Session,
                RuntimeExtensionAreaDto::Announcements,
                RuntimeExtensionAreaDto::Mcp,
                RuntimeExtensionAreaDto::Malformed
            ]
        );
        assert_values!(
            "authenticationMethod",
            [
                AuthenticationMethodDto::CachedToken,
                AuthenticationMethodDto::GrokCom,
                AuthenticationMethodDto::XaiApiKey,
                AuthenticationMethodDto::Grok,
                AuthenticationMethodDto::Other
            ]
        );
        assert_values!(
            "runtimeAgentProduct",
            [
                RuntimeAgentProductDto::GrokBuild,
                RuntimeAgentProductDto::Other
            ]
        );
        assert_values!(
            "executableState",
            [
                ExecutableStateDto::Available,
                ExecutableStateDto::Missing,
                ExecutableStateDto::Invalid
            ]
        );
        assert_values!(
            "executableSource",
            [
                ExecutableSourceDto::Configured,
                ExecutableSourceDto::UserInstall,
                ExecutableSourceDto::Path
            ]
        );
        assert_values!(
            "promptStopReason",
            [
                PromptStopReasonDto::EndTurn,
                PromptStopReasonDto::MaxTokens,
                PromptStopReasonDto::MaxTurnRequests,
                PromptStopReasonDto::Refusal,
                PromptStopReasonDto::Cancelled,
                PromptStopReasonDto::Other
            ]
        );
        assert_values!(
            "planEntryStatus",
            [
                PlanEntryStatusDto::Pending,
                PlanEntryStatusDto::InProgress,
                PlanEntryStatusDto::Completed
            ]
        );
        assert_values!(
            "observedExtensionClassification",
            [
                ObservedExtensionClassificationDto::Unknown,
                ObservedExtensionClassificationDto::Invalid
            ]
        );
        assert_values!(
            "interactionKind",
            [
                InteractionKindDto::Permission,
                InteractionKindDto::Elicitation
            ]
        );
        assert_values!(
            "applicationErrorCode",
            [
                ApplicationErrorCodeDto::ExecutableUnavailable,
                ApplicationErrorCodeDto::ProcessStartFailed,
                ApplicationErrorCodeDto::ConnectionFailed,
                ApplicationErrorCodeDto::RequestTimedOut,
                ApplicationErrorCodeDto::InitializeFailed,
                ApplicationErrorCodeDto::UnsupportedProtocol,
                ApplicationErrorCodeDto::UnsupportedAuthentication,
                ApplicationErrorCodeDto::AuthenticationFailed,
                ApplicationErrorCodeDto::RuntimeStopped,
                ApplicationErrorCodeDto::InvalidWorkspace,
                ApplicationErrorCodeDto::InvalidRequest,
                ApplicationErrorCodeDto::CapabilityUnavailable,
                ApplicationErrorCodeDto::ProtocolRequestFailed,
                ApplicationErrorCodeDto::MalformedResponse,
                ApplicationErrorCodeDto::UnknownInteraction,
                ApplicationErrorCodeDto::DecisionUnavailable,
                ApplicationErrorCodeDto::UnexpectedResponse,
                ApplicationErrorCodeDto::BoundaryViolation,
                ApplicationErrorCodeDto::PreferencesUnavailable
            ]
        );
        assert_eq!(enum_values.len(), 15, "every reviewed enum needs a fixture");

        assert_variant_fields!(
            "permissionScope",
            "type",
            [
                PermissionScopeDto::Tool {
                    tool_name: "tool".to_owned()
                },
                PermissionScopeDto::Command {
                    command: "run".to_owned(),
                    working_directory: None,
                    affected_paths: vec![],
                },
                PermissionScopeDto::Filesystem {
                    operation: "read".to_owned(),
                    path: "C:\\work".to_owned()
                },
                PermissionScopeDto::Network {
                    destination: "example.test".to_owned()
                },
                PermissionScopeDto::Other,
            ]
        );
        assert_variant_fields!(
            "elicitationControl",
            "type",
            [
                ElicitationControlDto::Text {
                    field_id: "text".to_owned(),
                    label: None,
                    placeholder: None,
                    sensitive: false,
                    min_length: 0,
                    max_length: 16384,
                },
                ElicitationControlDto::Confirmation {
                    field_id: "confirm".to_owned(),
                    label: None
                },
                ElicitationControlDto::Choice {
                    field_id: "choice".to_owned(),
                    label: None,
                    options: vec![],
                    multiple: false,
                    truncated: false
                },
                ElicitationControlDto::Other,
            ]
        );
        assert_variant_fields!(
            "configValue",
            "type",
            [
                ConfigValueDto::Select("high".to_owned()),
                ConfigValueDto::Boolean(true),
            ]
        );
        assert_variant_fields!(
            "elicitationDecision",
            "type",
            [
                ElicitationDecisionDto::Accept {
                    content: std::collections::BTreeMap::new()
                },
                ElicitationDecisionDto::Decline,
                ElicitationDecisionDto::Cancel,
            ]
        );
        assert_variant_fields!(
            "elicitationValue",
            "type",
            [
                ElicitationValueDto::String("text".to_owned()),
                ElicitationValueDto::Integer(1),
                ElicitationValueDto::Number(1.5),
                ElicitationValueDto::Boolean(true),
                ElicitationValueDto::StringArray(vec!["one".to_owned()]),
            ]
        );
        assert_variant_fields!(
            "optionalUpdate",
            "state",
            [
                OptionalUpdateDto::NotReported,
                OptionalUpdateDto::Cleared,
                OptionalUpdateDto::Value("value".to_owned()),
            ]
        );
        assert_variant_fields!(
            "configOptionKind",
            "type",
            [
                ConfigOptionKindDto::Select {
                    current_value: "high".to_owned(),
                    options: vec![],
                    truncated: false
                },
                ConfigOptionKindDto::Boolean {
                    current_value: true
                },
            ]
        );
        assert_eq!(variant_fields.len(), 7, "every tagged union needs fixtures");
    }

    #[test]
    fn unknown_extensions_collapse_without_method_names_or_payloads() {
        let runtime_event = grok_runtime::RuntimeEvent::ExtensionMethodObserved {
            session_id: Some("private-session".to_owned()),
            method: grok_runtime::ExtensionMethod::new("x.ai/unknown_extension")
                .expect("valid fixture method"),
        };

        let application_event = ApplicationEvent::from_runtime(runtime_event);
        let encoded = serde_json::to_string(&application_event).expect("event should serialize");

        assert_eq!(
            application_event,
            ApplicationEvent::ExtensionObserved {
                classification: ObservedExtensionClassificationDto::Unknown,
            }
        );
        assert!(!encoded.contains("unknown_extension"));
        assert!(!encoded.contains("private-session"));
        assert!(!encoded.contains("payload"));
    }

    #[test]
    fn projected_text_and_collections_are_bounded_on_utf8_boundaries() {
        let oversized_text = "🦀".repeat(MAX_EVENT_TEXT_BYTES);
        let message =
            ApplicationEvent::from_runtime(grok_runtime::RuntimeEvent::MessageChunkReceived {
                session_id: "session-1".to_owned(),
                message_id: Some("message-1".to_owned()),
                text: oversized_text,
            });
        let ApplicationEvent::MessageChunkReceived {
            text, truncated, ..
        } = message
        else {
            panic!("message event should remain a message event");
        };
        assert!(truncated);
        assert!(text.len() <= MAX_EVENT_TEXT_BYTES);
        assert!(text.is_char_boundary(text.len()));

        let entries = (0..(MAX_EVENT_COLLECTION_ITEMS + 10))
            .map(|index| grok_runtime::PlanEntry {
                id: format!("step-{index}"),
                title: format!("Step {index}"),
                description: None,
                status: grok_runtime::PlanEntryStatus::Pending,
            })
            .collect();
        let plan = ApplicationEvent::from_runtime(grok_runtime::RuntimeEvent::PlanChanged {
            session_id: "session-1".to_owned(),
            entries,
        });
        let ApplicationEvent::PlanChanged {
            entries, truncated, ..
        } = plan
        else {
            panic!("plan event should remain a plan event");
        };
        assert!(truncated);
        assert_eq!(entries.len(), MAX_EVENT_COLLECTION_ITEMS);
    }

    #[test]
    fn permission_projection_preserves_exact_choices_and_fails_closed_on_oversized_scope() {
        let event =
            ApplicationEvent::from_runtime(grok_runtime::RuntimeEvent::PermissionRequested {
                session_id: "session-1".to_owned(),
                interaction_id: "permission-1".to_owned(),
                title: "Run tests".to_owned(),
                consequence: None,
                kind: grok_runtime::PermissionKind::Command {
                    command: "cargo test".to_owned(),
                    working_directory: Some(std::path::PathBuf::from("C:\\workspace")),
                    affected_paths: vec![],
                },
                available_decisions: vec![
                    grok_runtime::PermissionDecision::AllowOnce,
                    grok_runtime::PermissionDecision::DenyAlways,
                ],
            });
        let ApplicationEvent::PermissionRequested {
            available_decisions,
            ..
        } = event
        else {
            panic!("valid permission should remain actionable");
        };
        assert_eq!(
            available_decisions,
            vec![
                PermissionDecisionDto::AllowOnce,
                PermissionDecisionDto::DenyAlways
            ]
        );

        let secret = "private-command".repeat(MAX_DISPLAY_TEXT_BYTES);
        let rejected =
            ApplicationEvent::from_runtime(grok_runtime::RuntimeEvent::PermissionRequested {
                session_id: "session-1".to_owned(),
                interaction_id: "permission-2".to_owned(),
                title: "Run command".to_owned(),
                consequence: None,
                kind: grok_runtime::PermissionKind::Command {
                    command: secret.clone(),
                    working_directory: None,
                    affected_paths: vec![],
                },
                available_decisions: vec![grok_runtime::PermissionDecision::AllowOnce],
            });
        let encoded = serde_json::to_string(&rejected).expect("failure should serialize");
        assert!(matches!(rejected, ApplicationEvent::RuntimeFailed { .. }));
        assert!(!encoded.contains(&secret));
    }

    #[test]
    fn inbound_requests_and_complete_events_have_hard_size_limits() {
        let prompt = PromptRequestDto {
            session_id: "session-1".to_owned(),
            text: "x".repeat(MAX_PROMPT_BYTES + 1),
        };
        assert_eq!(
            prompt
                .into_runtime_command()
                .expect_err("oversized prompt must not reach the runtime")
                .code,
            ApplicationErrorCodeDto::InvalidRequest
        );

        let permission = PermissionResponseRequestDto {
            interaction_id: "bad\nidentifier".to_owned(),
            decision: PermissionDecisionDto::DenyOnce,
        };
        assert_eq!(
            permission
                .validate()
                .expect_err("control characters must be rejected")
                .code,
            ApplicationErrorCodeDto::InvalidRequest
        );

        let clock = ApplicationEventClock::default();
        let envelope = clock.envelope(ApplicationEvent::SessionActivated {
            session_id: "x".repeat(MAX_SERIALIZED_EVENT_BYTES + 1),
        });
        assert!(matches!(
            envelope.event,
            ApplicationEvent::RuntimeFailed { .. }
        ));
    }

    #[test]
    fn event_clock_sequences_each_runtime_generation_independently() {
        let clock = ApplicationEventClock::default();
        let first = clock.envelope_runtime(grok_runtime::RuntimeEvent::RuntimeStateChanged {
            state: grok_runtime::RuntimeState::Connecting,
        });
        let second = clock.envelope_runtime(grok_runtime::RuntimeEvent::RuntimeStateChanged {
            state: grok_runtime::RuntimeState::Ready,
        });
        assert_eq!((first.generation, first.sequence), (1, 1));
        assert_eq!((second.generation, second.sequence), (1, 2));

        let stale = clock.envelope_runtime(grok_runtime::RuntimeEvent::RuntimeFailed {
            diagnostic: grok_runtime::RedactedDiagnostic::new("old worker stopped"),
            recoverable: true,
        });
        assert_eq!((stale.generation, stale.sequence), (1, 3));

        let restarted = clock.envelope_runtime(grok_runtime::RuntimeEvent::RuntimeStateChanged {
            state: grok_runtime::RuntimeState::Connecting,
        });
        assert_eq!((restarted.generation, restarted.sequence), (2, 1));

        let encoded = serde_json::to_string(&restarted).expect("envelope should serialize");
        let decoded: ApplicationEventEnvelope =
            serde_json::from_str(&encoded).expect("envelope should deserialize");
        assert_eq!(decoded, restarted);
    }

    #[test]
    fn workspace_validation_returns_only_an_existing_canonical_directory() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let validated = WorkspaceDto::validate(WorkspaceRequestDto {
            path: workspace.path().to_string_lossy().into_owned(),
        })
        .expect("existing directory should validate");
        assert_eq!(
            std::path::PathBuf::from(&validated.path),
            workspace
                .path()
                .canonicalize()
                .expect("canonical workspace")
        );

        let missing = workspace.path().join("missing");
        let error = WorkspaceDto::validate(WorkspaceRequestDto {
            path: missing.to_string_lossy().into_owned(),
        })
        .expect_err("missing directory should fail closed");
        assert_eq!(error.code, ApplicationErrorCodeDto::InvalidWorkspace);
        assert!(!error.diagnostic.contains(&missing.to_string_lossy()[..]));
    }

    #[test]
    fn workspace_request_rejects_unbounded_or_control_bearing_paths_without_echoing_them() {
        for private_path in [
            format!("C:\\private\\{}", "x".repeat(MAX_WORKSPACE_PATH_BYTES)),
            "C:\\private\\secret\nworkspace".to_owned(),
        ] {
            let error = WorkspaceRequestDto {
                path: private_path.clone(),
            }
            .into_path()
            .expect_err("unsafe paths must fail before filesystem access");

            assert_eq!(error.code, ApplicationErrorCodeDto::InvalidWorkspace);
            assert!(!error.diagnostic.contains(&private_path));
        }
    }

    #[test]
    fn external_urls_accept_only_credential_free_http_or_https() {
        assert_eq!(
            validate_external_url("https://docs.x.ai/build/overview").expect("safe url"),
            "https://docs.x.ai/build/overview"
        );
        assert_eq!(
            validate_external_url("HTTP://example.com/path").expect("http is allowed"),
            "HTTP://example.com/path"
        );

        for rejected in [
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "https://user:secret@example.com",
            "https://",
            "https://example.com/has space",
            &format!("https://example.com/{}", "a".repeat(MAX_EXTERNAL_URL_BYTES)),
        ] {
            let error = validate_external_url(rejected).expect_err("unsafe URLs must fail closed");
            assert_eq!(error.code, ApplicationErrorCodeDto::InvalidRequest);
            assert!(!error.diagnostic.contains(rejected));
            assert!(!error.diagnostic.contains("secret"));
        }
    }

    #[test]
    fn new_session_request_maps_to_domain_intent_without_protocol_names() {
        let request = NewSessionRequestDto {
            workspace: "C:\\workspace".to_owned(),
        };

        assert_eq!(
            request
                .into_runtime_command()
                .expect("bounded request should map"),
            grok_runtime::RuntimeCommand::NewSession {
                workspace: std::path::PathBuf::from("C:\\workspace"),
            }
        );
    }
}
