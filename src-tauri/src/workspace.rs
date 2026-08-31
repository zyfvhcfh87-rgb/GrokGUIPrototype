use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_RECENT_WORKSPACES: usize = 8;
const MAX_WORKSPACE_PATH_BYTES: usize = 32 * 1_024;
const MAX_PREFERENCES_BYTES: u64 = 256 * 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecentWorkspace {
    pub path: PathBuf,
    pub available: bool,
}

#[derive(Debug)]
pub(crate) struct WorkspaceStore {
    storage_path: PathBuf,
    paths: Vec<PathBuf>,
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
        let (paths, preference_error) = load(&storage_path);
        Self {
            storage_path,
            paths,
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
        self.preference_error = persist(&self.storage_path, &self.paths).is_err();
        Ok(canonical)
    }

    pub fn remove(&mut self, path: &Path) -> Result<(), WorkspaceError> {
        let mut paths = self.paths.clone();
        paths.retain(|existing| !same_path(existing, path));
        if paths == self.paths {
            return Ok(());
        }
        self.paths = paths;
        self.preference_error = persist(&self.storage_path, &self.paths).is_err();
        Ok(())
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

#[derive(Debug, Deserialize, Serialize)]
struct StoredWorkspaces {
    version: u8,
    paths: Vec<PathBuf>,
}

fn load(storage_path: &Path) -> (Vec<PathBuf>, bool) {
    let metadata = match std::fs::metadata(storage_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), false);
        }
        Err(_) => return (Vec::new(), true),
    };
    if !metadata.is_file() || metadata.len() > MAX_PREFERENCES_BYTES {
        return (Vec::new(), true);
    }
    let Ok(contents) = std::fs::read(storage_path) else {
        return (Vec::new(), true);
    };
    let Ok(stored) = serde_json::from_slice::<StoredWorkspaces>(&contents) else {
        return (Vec::new(), true);
    };
    if stored.version != 1 {
        return (Vec::new(), true);
    }

    let had_unsafe_path = stored
        .paths
        .iter()
        .any(|path| !is_safe_workspace_path(path));
    let paths = stored
        .paths
        .into_iter()
        .filter(|path| is_safe_workspace_path(path))
        .take(MAX_RECENT_WORKSPACES)
        .collect();
    (paths, had_unsafe_path)
}

fn persist(storage_path: &Path, paths: &[PathBuf]) -> Result<(), WorkspaceError> {
    let parent = storage_path
        .parent()
        .ok_or(WorkspaceError::PreferencesUnavailable)?;
    std::fs::create_dir_all(parent).map_err(|_| WorkspaceError::PreferencesUnavailable)?;
    let contents = serde_json::to_vec(&StoredWorkspaces {
        version: 1,
        paths: paths.to_vec(),
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
            }]
        );
        assert_eq!(
            WorkspaceStore::open(storage_path)
                .recent()
                .expect("reopened recent workspaces"),
            vec![RecentWorkspace {
                path: selected,
                available: true,
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
}
