use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const MAX_RECENT_WORKSPACES: usize = 8;
const MAX_WORKSPACE_PATH_BYTES: usize = 32 * 1_024;
const MAX_SESSION_ID_BYTES: usize = 256;
const MAX_PREFERENCES_BYTES: u64 = 256 * 1_024;
const PREFERENCES_VERSION: u8 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecentWorkspace {
    pub path: PathBuf,
    pub available: bool,
    pub last_session_id: Option<String>,
}

#[derive(Debug)]
pub(crate) struct WorkspaceStore {
    storage_path: PathBuf,
    paths: Vec<PathBuf>,
    last_sessions: Vec<(PathBuf, String)>,
    preference_error: bool,
}

#[derive(Debug)]
pub(crate) enum WorkspaceError {
    InvalidWorkspace,
    PreferencesUnavailable,
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidWorkspace => "select an existing workspace directory",
            Self::PreferencesUnavailable => "recent workspaces could not be saved",
        })
    }
}

impl std::error::Error for WorkspaceError {}

impl WorkspaceStore {
    pub fn open(storage_path: PathBuf) -> Self {
        let (paths, last_sessions, preference_error) = load(&storage_path);
        Self {
            storage_path,
            paths,
            last_sessions,
            preference_error,
        }
    }

    pub fn recent(&self) -> Result<Vec<RecentWorkspace>, WorkspaceError> {
        if self.preference_error {
            return Err(WorkspaceError::PreferencesUnavailable);
        }
        Ok(self
            .paths
            .iter()
            .take(MAX_RECENT_WORKSPACES)
            .cloned()
            .map(|path| RecentWorkspace {
                last_session_id: last_session_for(&self.last_sessions, &path),
                available: path.is_dir(),
                path,
            })
            .collect())
    }

    pub fn select(&mut self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        let canonical = canonicalize_workspace(path)?;

        let mut paths = self.paths.clone();
        paths.retain(|existing| !same_path(existing, &canonical));
        paths.insert(0, canonical.clone());
        paths.truncate(MAX_RECENT_WORKSPACES);
        self.paths = paths;
        self.prune_last_sessions();
        self.preference_error = self.persist().is_err();
        Ok(canonical)
    }

    pub fn remove(&mut self, path: &Path) -> Result<(), WorkspaceError> {
        let mut paths = self.paths.clone();
        paths.retain(|existing| !same_path(existing, path));
        if paths == self.paths {
            return Ok(());
        }
        self.paths = paths;
        self.prune_last_sessions();
        self.preference_error = self.persist().is_err();
        Ok(())
    }

    pub fn remember_session(&mut self, workspace: &Path, session_id: &str) {
        if !is_safe_session_id(session_id) {
            return;
        }
        let Some(path) = self
            .paths
            .iter()
            .find(|existing| same_path(existing, workspace))
            .cloned()
        else {
            return;
        };
        self.last_sessions
            .retain(|(existing, _)| !same_path(existing, &path));
        self.last_sessions.push((path, session_id.to_owned()));
        self.preference_error = self.persist().is_err();
    }

    pub fn forget_session(&mut self, session_id: &str) {
        let previous = self.last_sessions.len();
        self.last_sessions
            .retain(|(_, remembered)| remembered != session_id);
        if self.last_sessions.len() == previous {
            return;
        }
        self.preference_error = self.persist().is_err();
    }

    fn prune_last_sessions(&mut self) {
        self.last_sessions.retain(|(path, session_id)| {
            is_safe_session_id(session_id)
                && self.paths.iter().any(|existing| same_path(existing, path))
        });
    }

    fn persist(&self) -> Result<(), WorkspaceError> {
        persist(&self.storage_path, &self.paths, &self.last_sessions)
    }
}

pub(crate) fn canonicalize_workspace(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let canonical = path
        .canonicalize()
        .map_err(|_| WorkspaceError::InvalidWorkspace)?;
    if !canonical.is_dir() || !is_safe_workspace_path(&canonical) {
        return Err(WorkspaceError::InvalidWorkspace);
    }
    Ok(canonical)
}

fn is_safe_workspace_path(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|path| {
            !path.is_empty()
                && path.len() <= MAX_WORKSPACE_PATH_BYTES
                && !path.chars().any(char::is_control)
        })
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn is_safe_session_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_SESSION_ID_BYTES && !value.chars().any(char::is_control)
}

fn last_session_for(last_sessions: &[(PathBuf, String)], path: &Path) -> Option<String> {
    last_sessions
        .iter()
        .find(|(existing, _)| same_path(existing, path))
        .map(|(_, session_id)| session_id.clone())
}

#[derive(Debug, Deserialize, Serialize)]
struct StoredWorkspaces {
    version: u8,
    paths: Vec<PathBuf>,
    #[serde(default, rename = "lastSessions")]
    last_sessions: BTreeMap<String, String>,
}

fn load(storage_path: &Path) -> (Vec<PathBuf>, Vec<(PathBuf, String)>, bool) {
    let metadata = match std::fs::metadata(storage_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Vec::new(), false);
        }
        Err(_) => return (Vec::new(), Vec::new(), true),
    };
    if !metadata.is_file() || metadata.len() > MAX_PREFERENCES_BYTES {
        return (Vec::new(), Vec::new(), true);
    }
    let Ok(contents) = std::fs::read(storage_path) else {
        return (Vec::new(), Vec::new(), true);
    };
    let Ok(stored) = serde_json::from_slice::<StoredWorkspaces>(&contents) else {
        return (Vec::new(), Vec::new(), true);
    };
    if stored.version != 1 && stored.version != PREFERENCES_VERSION {
        return (Vec::new(), Vec::new(), true);
    }

    let had_unsafe_path = stored
        .paths
        .iter()
        .any(|path| !is_safe_workspace_path(path));
    let paths: Vec<PathBuf> = stored
        .paths
        .into_iter()
        .filter(|path| is_safe_workspace_path(path))
        .take(MAX_RECENT_WORKSPACES)
        .collect();
    let last_sessions = if stored.version == 1 || had_unsafe_path {
        Vec::new()
    } else {
        stored
            .last_sessions
            .into_iter()
            .filter_map(|(path, session_id)| {
                let path = PathBuf::from(path);
                if is_safe_workspace_path(&path)
                    && is_safe_session_id(&session_id)
                    && paths.iter().any(|existing| same_path(existing, &path))
                {
                    Some((path, session_id))
                } else {
                    None
                }
            })
            .collect()
    };
    (paths, last_sessions, had_unsafe_path)
}

fn persist(
    storage_path: &Path,
    paths: &[PathBuf],
    last_sessions: &[(PathBuf, String)],
) -> Result<(), WorkspaceError> {
    let parent = storage_path
        .parent()
        .ok_or(WorkspaceError::PreferencesUnavailable)?;
    std::fs::create_dir_all(parent).map_err(|_| WorkspaceError::PreferencesUnavailable)?;
    let stored_sessions = last_sessions
        .iter()
        .filter_map(|(path, session_id)| {
            path.to_str()
                .filter(|path| is_safe_workspace_path(Path::new(path)))
                .filter(|_| is_safe_session_id(session_id))
                .map(|path| (path.to_owned(), session_id.clone()))
        })
        .collect();
    let contents = serde_json::to_vec(&StoredWorkspaces {
        version: PREFERENCES_VERSION,
        paths: paths.to_vec(),
        last_sessions: stored_sessions,
    })
    .map_err(|_| WorkspaceError::PreferencesUnavailable)?;
    std::fs::write(storage_path, contents).map_err(|_| WorkspaceError::PreferencesUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_a_workspace_canonicalizes_records_and_reopens_it() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        let storage_path = root.path().join("preferences").join("workspaces.json");

        let mut store = WorkspaceStore::open(storage_path.clone());
        let selected = store.select(&workspace).expect("select workspace");

        assert_eq!(
            selected,
            workspace.canonicalize().expect("canonical workspace")
        );
        assert_eq!(
            store.recent().expect("recent workspaces"),
            vec![RecentWorkspace {
                path: selected.clone(),
                available: true,
                last_session_id: None,
            }]
        );
        assert_eq!(
            WorkspaceStore::open(storage_path)
                .recent()
                .expect("reopened recent workspaces"),
            vec![RecentWorkspace {
                path: selected,
                available: true,
                last_session_id: None,
            }]
        );
    }

    #[test]
    fn a_stale_recent_workspace_stays_visible_and_can_be_removed() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        let storage_path = root.path().join("preferences").join("workspaces.json");
        let mut store = WorkspaceStore::open(storage_path.clone());
        let selected = store.select(&workspace).expect("select workspace");
        std::fs::remove_dir(&selected).expect("make recent workspace stale");

        let mut reopened = WorkspaceStore::open(storage_path.clone());
        assert_eq!(
            reopened.recent().expect("stale recent workspace"),
            vec![RecentWorkspace {
                path: selected.clone(),
                available: false,
                last_session_id: None,
            }]
        );

        reopened.remove(&selected).expect("remove stale workspace");
        assert!(
            reopened
                .recent()
                .expect("removed recent workspace")
                .is_empty()
        );
        assert!(
            WorkspaceStore::open(storage_path)
                .recent()
                .expect("persisted removal")
                .is_empty()
        );
    }

    #[test]
    fn tampered_preferences_cannot_inject_relative_or_unbounded_paths() {
        let root = tempfile::tempdir().expect("temporary root");
        let storage_path = root.path().join("workspaces.json");
        let absolute = root.path().join("workspace");
        std::fs::create_dir(&absolute).expect("workspace directory");
        let stored = StoredWorkspaces {
            version: 1,
            paths: vec![
                PathBuf::from("relative-workspace"),
                PathBuf::from(format!("C:\\{}", "x".repeat(MAX_WORKSPACE_PATH_BYTES))),
                absolute.canonicalize().expect("canonical workspace"),
            ],
            last_sessions: BTreeMap::new(),
        };
        std::fs::write(
            &storage_path,
            serde_json::to_vec(&stored).expect("serialize fixture"),
        )
        .expect("write fixture");

        assert!(WorkspaceStore::open(storage_path).recent().is_err());
    }

    #[test]
    fn preference_write_failure_does_not_reject_a_valid_workspace() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        let blocked_parent = root.path().join("not-a-directory");
        std::fs::write(&blocked_parent, b"fixture").expect("blocking file");
        let mut store = WorkspaceStore::open(blocked_parent.join("workspaces.json"));

        let selected = store
            .select(&workspace)
            .expect("valid workspace selection should survive preference failure");

        assert_eq!(
            selected,
            workspace.canonicalize().expect("canonical workspace")
        );
        assert!(store.recent().is_err());
    }

    #[test]
    fn malformed_preferences_surface_an_error_instead_of_looking_empty() {
        let root = tempfile::tempdir().expect("temporary root");
        let storage_path = root.path().join("workspaces.json");
        std::fs::write(&storage_path, b"not json").expect("malformed fixture");

        assert!(WorkspaceStore::open(storage_path).recent().is_err());
    }

    #[test]
    fn version_one_preferences_migrate_and_remember_a_session_id() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        let canonical = workspace.canonicalize().expect("canonical workspace");
        let storage_path = root.path().join("workspaces.json");
        std::fs::write(
            &storage_path,
            serde_json::to_vec(&StoredWorkspaces {
                version: 1,
                paths: vec![canonical.clone()],
                last_sessions: BTreeMap::new(),
            })
            .expect("serialize version 1 fixture"),
        )
        .expect("write version 1 fixture");

        let mut store = WorkspaceStore::open(storage_path.clone());
        assert_eq!(
            store.recent().expect("migrated recents")[0].last_session_id,
            None
        );
        store.remember_session(&canonical, "session-001");

        assert_eq!(
            store.recent().expect("remembered session")[0]
                .last_session_id
                .as_deref(),
            Some("session-001")
        );
        let reopened = WorkspaceStore::open(storage_path.clone());
        assert_eq!(
            reopened.recent().expect("reopened v2 recents")[0]
                .last_session_id
                .as_deref(),
            Some("session-001")
        );
        let stored: StoredWorkspaces =
            serde_json::from_slice(&std::fs::read(&storage_path).expect("read persisted prefs"))
                .expect("persisted prefs should be JSON");
        assert_eq!(stored.version, PREFERENCES_VERSION);
    }

    #[test]
    fn invalid_session_ids_are_dropped_without_discarding_recents() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        let canonical = workspace.canonicalize().expect("canonical workspace");
        let storage_path = root.path().join("workspaces.json");
        let mut last_sessions = BTreeMap::new();
        last_sessions.insert(
            canonical.to_string_lossy().into_owned(),
            "session\u{0007}-bad".to_owned(),
        );
        std::fs::write(
            &storage_path,
            serde_json::to_vec(&StoredWorkspaces {
                version: PREFERENCES_VERSION,
                paths: vec![canonical.clone()],
                last_sessions,
            })
            .expect("serialize fixture"),
        )
        .expect("write fixture");

        let recents = WorkspaceStore::open(storage_path)
            .recent()
            .expect("recents should remain usable");
        assert_eq!(
            recents,
            vec![RecentWorkspace {
                path: canonical,
                available: true,
                last_session_id: None,
            }]
        );
    }

    #[test]
    fn closing_a_session_forgets_it_and_removing_a_workspace_drops_its_session() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        let storage_path = root.path().join("workspaces.json");
        let mut store = WorkspaceStore::open(storage_path.clone());
        let selected = store.select(&workspace).expect("select workspace");
        store.remember_session(&selected, "session-001");
        store.forget_session("session-001");
        assert_eq!(
            store.recent().expect("forgotten session")[0].last_session_id,
            None
        );

        store.remember_session(&selected, "session-002");
        store.remove(&selected).expect("remove workspace");
        assert!(store.recent().expect("removed workspace").is_empty());
        assert!(
            WorkspaceStore::open(storage_path)
                .recent()
                .expect("persisted removal")
                .is_empty()
        );
    }

    #[test]
    fn remember_session_survives_preference_write_failure() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        let blocked_parent = root.path().join("not-a-directory");
        std::fs::write(&blocked_parent, b"fixture").expect("blocking file");
        let mut store = WorkspaceStore::open(blocked_parent.join("workspaces.json"));
        let selected = store.select(&workspace).expect("select workspace");
        store.remember_session(&selected, "session-001");
        assert!(store.recent().is_err());
    }
}
