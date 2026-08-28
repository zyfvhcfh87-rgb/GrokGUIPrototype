use std::{
    env,
    ffi::{OsStr, OsString},
    fmt, fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

/// Environment variable used to override executable discovery.
pub const GROK_PATH_ENV: &str = "GROK_BUILD_GUI_GROK_PATH";

/// Arguments for launching Grok Build's ACP v1 stdio mode directly.
pub const GROK_STDIO_ARGS: [&str; 4] = ["--no-auto-update", "agent", "--no-leader", "stdio"];

/// Where the resolved executable was found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrokExecutableSource {
    Explicit,
    Environment,
    UserInstall,
    Path,
}

impl fmt::Display for GrokExecutableSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Explicit => "explicit path",
            Self::Environment => GROK_PATH_ENV,
            Self::UserInstall => "the default user installation",
            Self::Path => "PATH",
        };

        formatter.write_str(label)
    }
}

/// A canonical, regular-file path that is safe to pass directly to a process
/// launcher without involving a shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedGrokExecutable {
    path: PathBuf,
    source: GrokExecutableSource,
}

impl ResolvedGrokExecutable {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn source(&self) -> GrokExecutableSource {
        self.source
    }

    pub fn into_path(self) -> PathBuf {
        self.path
    }

    pub fn launch_args(&self) -> &'static [&'static str; 4] {
        &GROK_STDIO_ARGS
    }
}

#[derive(Debug, Error)]
pub enum ResolveGrokExecutableError {
    #[error("Grok executable from {origin} could not be resolved at `{path}`: {error}")]
    UnusableConfiguredPath {
        origin: GrokExecutableSource,
        path: PathBuf,
        #[source]
        error: io::Error,
    },

    #[error("Grok executable from {origin} is not a regular file: `{path}`")]
    NotARegularFile {
        origin: GrokExecutableSource,
        path: PathBuf,
    },

    #[error(
        "Grok Build was not found; provide an explicit path, set {GROK_PATH_ENV}, install it in the user .grok/bin directory, or add it to PATH"
    )]
    NotFound,
}

/// Resolve Grok Build in strict precedence order: an explicit path, the
/// application override, the expected per-user install, then `PATH`.
///
/// A supplied explicit path or non-empty override is authoritative. If it is
/// unusable, this returns an error rather than silently launching another
/// executable. Automatically discovered candidates may fall through to the
/// next source.
pub fn resolve_grok_executable(
    explicit_path: Option<&Path>,
) -> Result<ResolvedGrokExecutable, ResolveGrokExecutableError> {
    resolve_with_environment(explicit_path, &SearchEnvironment::current())
}

#[derive(Debug, Default)]
struct SearchEnvironment {
    configured_path: Option<OsString>,
    home: Option<PathBuf>,
    path: Option<OsString>,
}

impl SearchEnvironment {
    fn current() -> Self {
        Self {
            configured_path: env::var_os(GROK_PATH_ENV),
            home: current_home_dir(),
            path: env::var_os("PATH"),
        }
    }
}

pub(crate) fn current_home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(profile) = non_empty_env("USERPROFILE") {
            return Some(PathBuf::from(profile));
        }

        if let (Some(drive), Some(path)) = (non_empty_env("HOMEDRIVE"), non_empty_env("HOMEPATH")) {
            let mut home = PathBuf::from(drive);
            home.push(path);
            return Some(home);
        }
    }

    non_empty_env("HOME").map(PathBuf::from)
}

fn non_empty_env(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn resolve_with_environment(
    explicit_path: Option<&Path>,
    environment: &SearchEnvironment,
) -> Result<ResolvedGrokExecutable, ResolveGrokExecutableError> {
    if let Some(path) = explicit_path {
        return resolve_authoritative(path, GrokExecutableSource::Explicit);
    }

    if let Some(path) = environment
        .configured_path
        .as_deref()
        .filter(|path| !path.is_empty())
    {
        return resolve_authoritative(Path::new(path), GrokExecutableSource::Environment);
    }

    if let Some(home) = &environment.home {
        let candidate = home.join(".grok").join("bin").join(grok_executable_name());

        if let Some(resolved) = resolve_discovered(&candidate, GrokExecutableSource::UserInstall) {
            return Ok(resolved);
        }
    }

    if let Some(path) = &environment.path {
        for directory in env::split_paths(path).filter(|entry| !entry.as_os_str().is_empty()) {
            let candidate = directory.join(grok_executable_name());
            if let Some(resolved) = resolve_discovered(&candidate, GrokExecutableSource::Path) {
                return Ok(resolved);
            }
        }
    }

    Err(ResolveGrokExecutableError::NotFound)
}

fn resolve_authoritative(
    path: &Path,
    source: GrokExecutableSource,
) -> Result<ResolvedGrokExecutable, ResolveGrokExecutableError> {
    let canonical_path = fs::canonicalize(path).map_err(|error| {
        ResolveGrokExecutableError::UnusableConfiguredPath {
            origin: source,
            path: path.to_path_buf(),
            error,
        }
    })?;

    if !canonical_path.is_file() {
        return Err(ResolveGrokExecutableError::NotARegularFile {
            origin: source,
            path: canonical_path,
        });
    }

    Ok(ResolvedGrokExecutable {
        path: canonical_path,
        source,
    })
}

fn resolve_discovered(path: &Path, source: GrokExecutableSource) -> Option<ResolvedGrokExecutable> {
    let canonical_path = fs::canonicalize(path).ok()?;
    canonical_path.is_file().then_some(ResolvedGrokExecutable {
        path: canonical_path,
        source,
    })
}

fn grok_executable_name() -> &'static OsStr {
    #[cfg(windows)]
    {
        OsStr::new("grok.exe")
    }

    #[cfg(not(windows))]
    {
        OsStr::new("grok")
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "grok-runtime-executable-tests-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create isolated test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn fake_grok(&self, relative_directory: &str) -> PathBuf {
            let directory = self.0.join(relative_directory);
            fs::create_dir_all(&directory).expect("create fake executable parent");
            let executable = directory.join(grok_executable_name());
            fs::write(&executable, b"fake grok executable").expect("write fake executable");
            executable
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn environment(
        configured_path: Option<PathBuf>,
        home: Option<PathBuf>,
        path_entries: &[PathBuf],
    ) -> SearchEnvironment {
        SearchEnvironment {
            configured_path: configured_path.map(PathBuf::into_os_string),
            home,
            path: (!path_entries.is_empty())
                .then(|| env::join_paths(path_entries).expect("join test PATH entries")),
        }
    }

    #[test]
    fn explicit_path_is_canonical_and_wins_every_other_source() {
        let temp = TestDirectory::new();
        let explicit = temp.fake_grok("explicit");
        let configured = temp.fake_grok("configured");
        let home = temp.path().join("home");
        let path_executable = temp.fake_grok("path");
        let inputs = environment(
            Some(configured),
            Some(home),
            &[path_executable.parent().unwrap().to_path_buf()],
        );

        let resolved = resolve_with_environment(Some(&explicit), &inputs).unwrap();

        assert_eq!(resolved.path(), fs::canonicalize(explicit).unwrap());
        assert_eq!(resolved.source(), GrokExecutableSource::Explicit);
    }

    #[test]
    fn invalid_configured_path_does_not_silently_fall_back() {
        let temp = TestDirectory::new();
        let fallback = temp.fake_grok("path");
        let missing = temp.path().join("missing-grok");
        let inputs = environment(
            Some(missing.clone()),
            None,
            &[fallback.parent().unwrap().to_path_buf()],
        );

        let error = resolve_with_environment(None, &inputs).unwrap_err();

        assert!(matches!(
            error,
            ResolveGrokExecutableError::UnusableConfiguredPath {
                origin: GrokExecutableSource::Environment,
                path,
                ..
            } if path == missing
        ));
    }

    #[test]
    fn default_user_install_precedes_path() {
        let temp = TestDirectory::new();
        let home = temp.path().join("home");
        let expected_parent = home.join(".grok").join("bin");
        fs::create_dir_all(&expected_parent).unwrap();
        let expected = expected_parent.join(grok_executable_name());
        fs::write(&expected, b"user install").unwrap();
        let path_executable = temp.fake_grok("path");
        let inputs = environment(
            None,
            Some(home),
            &[path_executable.parent().unwrap().to_path_buf()],
        );

        let resolved = resolve_with_environment(None, &inputs).unwrap();

        assert_eq!(resolved.path(), fs::canonicalize(expected).unwrap());
        assert_eq!(resolved.source(), GrokExecutableSource::UserInstall);
    }

    #[test]
    fn path_search_skips_directories_named_like_the_executable() {
        let temp = TestDirectory::new();
        let first = temp.path().join("first");
        fs::create_dir_all(first.join(grok_executable_name())).unwrap();
        let valid = temp.fake_grok("second");
        let inputs = environment(None, None, &[first, valid.parent().unwrap().to_path_buf()]);

        let resolved = resolve_with_environment(None, &inputs).unwrap();

        assert_eq!(resolved.path(), fs::canonicalize(valid).unwrap());
        assert_eq!(resolved.source(), GrokExecutableSource::Path);
    }

    #[test]
    fn launch_arguments_are_exact_and_shell_free() {
        let temp = TestDirectory::new();
        let executable = temp.fake_grok("explicit");
        let resolved =
            resolve_with_environment(Some(&executable), &SearchEnvironment::default()).unwrap();

        assert_eq!(
            resolved.launch_args(),
            &["--no-auto-update", "agent", "--no-leader", "stdio"]
        );
    }
}
