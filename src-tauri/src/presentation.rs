use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::application_contract::{
    OnboardingStatusDto, PresentationMotionDto, PresentationPreferencesDto, PresentationThemeDto,
};

const MAX_PREFERENCES_BYTES: u64 = 16 * 1_024;
const PREFERENCES_VERSION: u8 = 1;
const MIN_PANEL_WIDTH: u16 = 200;
const MAX_PANEL_WIDTH: u16 = 420;
const DEFAULT_PROJECTS_WIDTH: u16 = 260;
const DEFAULT_DETAILS_WIDTH: u16 = 280;

#[derive(Debug)]
pub(crate) struct PresentationStore {
    storage_path: PathBuf,
    preferences: PresentationPreferencesDto,
    preference_error: bool,
}

impl PresentationStore {
    pub fn open(storage_path: PathBuf) -> Self {
        let (preferences, preference_error) = load(&storage_path);
        Self {
            storage_path,
            preferences,
            preference_error,
        }
    }

    pub fn get(&self) -> Result<PresentationPreferencesDto, super::workspace::WorkspaceError> {
        if self.preference_error {
            return Err(super::workspace::WorkspaceError::PreferencesUnavailable);
        }
        Ok(self.preferences.clone())
    }

    pub fn set(
        &mut self,
        preferences: PresentationPreferencesDto,
    ) -> Result<PresentationPreferencesDto, super::workspace::WorkspaceError> {
        let sanitized = sanitize(preferences);
        self.preferences = sanitized.clone();
        self.preference_error = persist(&self.storage_path, &sanitized).is_err();
        if self.preference_error {
            return Err(super::workspace::WorkspaceError::PreferencesUnavailable);
        }
        Ok(sanitized)
    }
}

pub(crate) fn default_preferences() -> PresentationPreferencesDto {
    PresentationPreferencesDto {
        theme: PresentationThemeDto::System,
        motion: PresentationMotionDto::System,
        projects_open: true,
        details_open: true,
        projects_width: DEFAULT_PROJECTS_WIDTH,
        details_width: DEFAULT_DETAILS_WIDTH,
        onboarding: OnboardingStatusDto::Unseen,
    }
}

fn sanitize(value: PresentationPreferencesDto) -> PresentationPreferencesDto {
    PresentationPreferencesDto {
        theme: value.theme,
        motion: value.motion,
        projects_open: value.projects_open,
        details_open: value.details_open,
        projects_width: clamp_width(value.projects_width),
        details_width: clamp_width(value.details_width),
        onboarding: value.onboarding,
    }
}

fn clamp_width(value: u16) -> u16 {
    value.clamp(MIN_PANEL_WIDTH, MAX_PANEL_WIDTH)
}

#[derive(Debug, Deserialize, Serialize)]
struct StoredPresentation {
    version: u8,
    #[serde(flatten)]
    preferences: PresentationPreferencesDto,
}

fn load(storage_path: &Path) -> (PresentationPreferencesDto, bool) {
    let metadata = match std::fs::metadata(storage_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (default_preferences(), false);
        }
        Err(_) => return (default_preferences(), true),
    };
    if !metadata.is_file() || metadata.len() > MAX_PREFERENCES_BYTES {
        return (default_preferences(), true);
    }
    let Ok(contents) = std::fs::read(storage_path) else {
        return (default_preferences(), true);
    };
    let Ok(stored) = serde_json::from_slice::<StoredPresentation>(&contents) else {
        return (default_preferences(), true);
    };
    if stored.version != PREFERENCES_VERSION {
        return (default_preferences(), true);
    }
    (sanitize(stored.preferences), false)
}

fn persist(
    storage_path: &Path,
    preferences: &PresentationPreferencesDto,
) -> Result<(), super::workspace::WorkspaceError> {
    let parent = storage_path
        .parent()
        .ok_or(super::workspace::WorkspaceError::PreferencesUnavailable)?;
    std::fs::create_dir_all(parent)
        .map_err(|_| super::workspace::WorkspaceError::PreferencesUnavailable)?;
    let contents = serde_json::to_vec(&StoredPresentation {
        version: PREFERENCES_VERSION,
        preferences: sanitize(preferences.clone()),
    })
    .map_err(|_| super::workspace::WorkspaceError::PreferencesUnavailable)?;
    if contents.len() as u64 > MAX_PREFERENCES_BYTES {
        return Err(super::workspace::WorkspaceError::PreferencesUnavailable);
    }
    std::fs::write(storage_path, contents)
        .map_err(|_| super::workspace::WorkspaceError::PreferencesUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_uses_defaults_without_error() {
        let root = tempfile::tempdir().expect("temporary root");
        let store = PresentationStore::open(root.path().join("presentation.json"));
        assert_eq!(store.get().expect("defaults"), default_preferences());
    }

    #[test]
    fn round_trip_persists_theme_layout_and_onboarding() {
        let root = tempfile::tempdir().expect("temporary root");
        let storage_path = root.path().join("presentation.json");
        let mut store = PresentationStore::open(storage_path.clone());
        let saved = store
            .set(PresentationPreferencesDto {
                theme: PresentationThemeDto::Light,
                motion: PresentationMotionDto::Reduce,
                projects_open: false,
                details_open: true,
                projects_width: 300,
                details_width: 240,
                onboarding: OnboardingStatusDto::Skipped,
            })
            .expect("save presentation");

        assert_eq!(saved.theme, PresentationThemeDto::Light);
        assert_eq!(saved.onboarding, OnboardingStatusDto::Skipped);
        assert_eq!(
            PresentationStore::open(storage_path)
                .get()
                .expect("reopened")
                .motion,
            PresentationMotionDto::Reduce
        );
    }

    #[test]
    fn out_of_range_widths_are_clamped() {
        let sanitized = sanitize(PresentationPreferencesDto {
            theme: PresentationThemeDto::Dark,
            motion: PresentationMotionDto::System,
            projects_open: true,
            details_open: false,
            projects_width: 12,
            details_width: 9_000,
            onboarding: OnboardingStatusDto::Completed,
        });
        assert_eq!(sanitized.projects_width, MIN_PANEL_WIDTH);
        assert_eq!(sanitized.details_width, MAX_PANEL_WIDTH);
    }

    #[test]
    fn malformed_preferences_surface_an_error_instead_of_looking_empty() {
        let root = tempfile::tempdir().expect("temporary root");
        let storage_path = root.path().join("presentation.json");
        std::fs::write(&storage_path, b"not json").expect("malformed fixture");
        assert!(PresentationStore::open(storage_path).get().is_err());
    }

    #[test]
    fn skipped_onboarding_can_be_reopened_without_resetting_theme() {
        let root = tempfile::tempdir().expect("temporary root");
        let storage_path = root.path().join("presentation.json");
        let mut store = PresentationStore::open(storage_path.clone());
        store
            .set(PresentationPreferencesDto {
                theme: PresentationThemeDto::Dark,
                motion: PresentationMotionDto::System,
                projects_open: true,
                details_open: true,
                projects_width: 260,
                details_width: 280,
                onboarding: OnboardingStatusDto::Skipped,
            })
            .expect("skip");
        let reopened = store
            .set(PresentationPreferencesDto {
                theme: PresentationThemeDto::Dark,
                motion: PresentationMotionDto::System,
                projects_open: true,
                details_open: true,
                projects_width: 260,
                details_width: 280,
                onboarding: OnboardingStatusDto::Unseen,
            })
            .expect("reopen");
        assert_eq!(reopened.onboarding, OnboardingStatusDto::Unseen);
        assert_eq!(reopened.theme, PresentationThemeDto::Dark);
    }
}
