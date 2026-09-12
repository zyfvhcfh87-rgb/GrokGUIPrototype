use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use grok_runtime::RedactedDiagnostic;
use tokio::process::Command;

use crate::{
    application_contract::{
        ApplicationErrorDto, WorkspaceChangeContentDto, WorkspaceChangeEntryDto,
        WorkspaceChangeKindDto, WorkspaceChangeStatusDto, WorkspaceChangesDto,
    },
    workspace::canonicalize_workspace,
};

const GIT_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_CHANGED_PATHS: usize = 64;
const MAX_DIFF_LINES_PER_FILE: usize = 200;
const MAX_DIFF_BYTES_PER_FILE: usize = 8 * 1_024;
const MAX_TOTAL_DIFF_BYTES: usize = 32 * 1_024;
const MAX_RELATIVE_PATH_BYTES: usize = 4 * 1_024;
const MAX_GIT_OUTPUT_BYTES: usize = 256 * 1_024;

const GIT_NULL_CONFIG: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

const READ_ONLY_STATUS_ARGS: &[&str] = &[
    "status",
    "--porcelain=v1",
    "-z",
    "--untracked-files=all",
    "--ignored=matching",
];

pub async fn inspect_workspace_changes(
    path: &Path,
) -> Result<WorkspaceChangesDto, ApplicationErrorDto> {
    let workspace = canonicalize_workspace(path).map_err(ApplicationErrorDto::from)?;
    inspect_canonical_workspace(&workspace).await
}

async fn inspect_canonical_workspace(
    workspace: &Path,
) -> Result<WorkspaceChangesDto, ApplicationErrorDto> {
    let inside = match git_output(workspace, &["rev-parse", "--is-inside-work-tree"]).await {
        Ok(output) if output.status_success && output.stdout.trim() == "true" => true,
        Ok(_) => false,
        Err(GitInvokeError::NotFound) => {
            return Ok(WorkspaceChangesDto::unavailable());
        }
        Err(GitInvokeError::TimedOut | GitInvokeError::Failed) => {
            return Ok(WorkspaceChangesDto::unavailable());
        }
    };
    if !inside {
        return Ok(WorkspaceChangesDto::not_a_repository());
    }

    let toplevel = match git_output(workspace, &["rev-parse", "--show-toplevel"]).await {
        Ok(output) if output.status_success => {
            let trimmed = output.stdout.trim();
            if trimmed.is_empty() {
                return Ok(WorkspaceChangesDto::not_a_repository());
            }
            PathBuf::from(trimmed)
        }
        _ => return Ok(WorkspaceChangesDto::not_a_repository()),
    };
    let toplevel = match toplevel.canonicalize() {
        Ok(path) if path_is_within(workspace, &path) || path_is_within(&path, workspace) => path,
        _ => return Ok(WorkspaceChangesDto::unavailable()),
    };

    let status = match git_output(&toplevel, READ_ONLY_STATUS_ARGS).await {
        Ok(output) if output.status_success => output.stdout_bytes,
        Ok(_) | Err(_) => return Ok(WorkspaceChangesDto::unavailable()),
    };

    let parsed = parse_porcelain_status(&status);
    let mut omitted_entry_count = 0_u32;
    let mut omitted_line_count = 0_u32;
    let mut truncated = parsed.truncated;
    let mut total_diff_bytes = 0_usize;
    let mut entries = Vec::new();

    for record in parsed.entries {
        if entries.len() >= MAX_CHANGED_PATHS {
            omitted_entry_count = omitted_entry_count.saturating_add(1);
            truncated = true;
            continue;
        }
        let Some(relative) = display_path_in_workspace(workspace, &toplevel, &record.path) else {
            continue;
        };
        let previous_path = record
            .previous_path
            .as_deref()
            .and_then(|path| display_path_in_workspace(workspace, &toplevel, path));
        if record.previous_path.is_some() && previous_path.is_none() {
            continue;
        }

        let mut entry = WorkspaceChangeEntryDto {
            path: relative,
            previous_path,
            status: record.status,
            content: WorkspaceChangeContentDto::Omitted,
            diff: None,
            truncated: false,
        };

        if should_omit_diff(&entry.path, entry.status) {
            entries.push(entry);
            continue;
        }

        match diff_for_path(&toplevel, &record.path).await {
            DiffInspection::Text {
                text,
                truncated: diff_truncated,
                omitted_lines,
            } => {
                omitted_line_count = omitted_line_count.saturating_add(omitted_lines);
                let remaining = MAX_TOTAL_DIFF_BYTES.saturating_sub(total_diff_bytes);
                if remaining == 0 {
                    entry.content = WorkspaceChangeContentDto::Omitted;
                    entry.truncated = true;
                    truncated = true;
                } else {
                    let (bounded, budget_truncated) = bound_diff_text(&text, remaining);
                    total_diff_bytes = total_diff_bytes.saturating_add(bounded.len());
                    entry.content = WorkspaceChangeContentDto::Text;
                    entry.diff = Some(bounded);
                    entry.truncated = diff_truncated || budget_truncated;
                    truncated |= entry.truncated;
                }
            }
            DiffInspection::Binary => {
                entry.content = WorkspaceChangeContentDto::Binary;
            }
            DiffInspection::Unavailable => {
                entry.content = WorkspaceChangeContentDto::Unavailable;
            }
        }
        entries.push(entry);
    }

    Ok(WorkspaceChangesDto {
        kind: WorkspaceChangeKindDto::Repository,
        attributable_to_session: false,
        entries,
        truncated,
        omitted_entry_count,
        omitted_line_count,
    })
}

#[derive(Debug)]
enum GitInvokeError {
    NotFound,
    TimedOut,
    Failed,
}

struct GitOutput {
    status_success: bool,
    stdout: String,
    stdout_bytes: Vec<u8>,
}

async fn git_output(workspace: &Path, args: &[&str]) -> Result<GitOutput, GitInvokeError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(workspace);
    command.arg("--no-pager");
    command.arg("--no-optional-locks");
    command.args(args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_PAGER", "cat");
    command.env("PAGER", "cat");
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_GLOBAL", GIT_NULL_CONFIG);
    command.env_remove("GIT_DIR");
    command.env_remove("GIT_WORK_TREE");
    command.kill_on_drop(true);

    let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| GitInvokeError::TimedOut)?
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                GitInvokeError::NotFound
            } else {
                GitInvokeError::Failed
            }
        })?;

    if output.stdout.len() > MAX_GIT_OUTPUT_BYTES {
        return Err(GitInvokeError::Failed);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let _ = RedactedDiagnostic::new(String::from_utf8_lossy(&output.stderr));
    Ok(GitOutput {
        status_success: output.status.success(),
        stdout,
        stdout_bytes: output.stdout,
    })
}

struct PorcelainRecord {
    status: WorkspaceChangeStatusDto,
    path: String,
    previous_path: Option<String>,
}

struct ParsedStatus {
    entries: Vec<PorcelainRecord>,
    truncated: bool,
}

fn parse_porcelain_status(bytes: &[u8]) -> ParsedStatus {
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes.len() - cursor < 3 {
            truncated = true;
            break;
        }
        let xy = [bytes[cursor], bytes[cursor + 1]];
        cursor += 2;
        if cursor < bytes.len() && bytes[cursor] == b' ' {
            cursor += 1;
        }
        let Some(path) = read_nul_field(bytes, &mut cursor) else {
            truncated = true;
            break;
        };
        let status = classify_status(xy);
        let previous_path = if matches!(
            status,
            WorkspaceChangeStatusDto::Renamed | WorkspaceChangeStatusDto::Copied
        ) {
            read_nul_field(bytes, &mut cursor)
        } else {
            None
        };
        entries.push(PorcelainRecord {
            status,
            path,
            previous_path,
        });
    }
    ParsedStatus { entries, truncated }
}

fn read_nul_field(bytes: &[u8], cursor: &mut usize) -> Option<String> {
    let start = *cursor;
    while *cursor < bytes.len() && bytes[*cursor] != 0 {
        *cursor += 1;
    }
    if *cursor >= bytes.len() {
        return None;
    }
    let slice = &bytes[start..*cursor];
    *cursor += 1;
    let text = std::str::from_utf8(slice).ok()?;
    if text.is_empty() || text.len() > MAX_RELATIVE_PATH_BYTES || text.chars().any(char::is_control)
    {
        return None;
    }
    Some(text.to_owned())
}

fn classify_status(xy: [u8; 2]) -> WorkspaceChangeStatusDto {
    let index = if xy[0] == b'?' && xy[1] == b'?' {
        b'?'
    } else if xy[0] == b'!' && xy[1] == b'!' {
        b'!'
    } else if xy[0] == b'U' || xy[1] == b'U' || (xy[0] == b'A' && xy[1] == b'A') {
        b'U'
    } else if xy[0] == b'R' || xy[1] == b'R' {
        b'R'
    } else if xy[0] == b'C' || xy[1] == b'C' {
        b'C'
    } else if xy[0] == b'D' || xy[1] == b'D' {
        b'D'
    } else if xy[0] == b'A' || xy[1] == b'A' || xy[0] == b'?' {
        b'A'
    } else if xy[0] == b'M' || xy[1] == b'M' {
        b'M'
    } else {
        b' '
    };
    match index {
        b'A' => WorkspaceChangeStatusDto::Added,
        b'M' => WorkspaceChangeStatusDto::Modified,
        b'D' => WorkspaceChangeStatusDto::Deleted,
        b'R' => WorkspaceChangeStatusDto::Renamed,
        b'C' => WorkspaceChangeStatusDto::Copied,
        b'U' => WorkspaceChangeStatusDto::Unmerged,
        b'?' => WorkspaceChangeStatusDto::Untracked,
        b'!' => WorkspaceChangeStatusDto::Ignored,
        _ => WorkspaceChangeStatusDto::Other,
    }
}

fn display_path_in_workspace(workspace: &Path, toplevel: &Path, git_path: &str) -> Option<String> {
    let relative = normalize_git_path(git_path)?;
    let joined = toplevel.join(&relative);
    if !path_is_within(workspace, &joined) {
        return None;
    }
    let displayed = joined.strip_prefix(workspace).ok()?;
    let text = displayed.to_str()?;
    if text.is_empty() || text.len() > MAX_RELATIVE_PATH_BYTES {
        return None;
    }
    Some(text.replace('\\', "/"))
}

fn normalize_git_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() || Path::new(path).is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(value) if !value.is_empty() && value != OsStr::new("..") => {
                out.push(value);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn path_is_within(boundary: &Path, candidate: &Path) -> bool {
    let mut current = Some(candidate);
    while let Some(path) = current {
        if paths_equal(path, boundary) {
            return true;
        }
        current = path.parent();
    }
    false
}

fn paths_equal(left: &Path, right: &Path) -> bool {
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

fn should_omit_diff(relative: &str, status: WorkspaceChangeStatusDto) -> bool {
    matches!(
        status,
        WorkspaceChangeStatusDto::Untracked
            | WorkspaceChangeStatusDto::Ignored
            | WorkspaceChangeStatusDto::Unmerged
            | WorkspaceChangeStatusDto::Other
    ) || is_sensitive_relative_path(relative)
}

fn is_sensitive_relative_path(relative: &str) -> bool {
    let normalized = relative.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let file_name = Path::new(&lower)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(&lower);
    file_name == "auth.json"
        || file_name == ".env"
        || file_name.starts_with(".env.")
        || file_name == "id_rsa"
        || file_name == "id_dsa"
        || file_name == "id_ecdsa"
        || file_name == "id_ed25519"
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
        || file_name.ends_with(".p12")
        || file_name.ends_with(".pfx")
        || file_name.contains("credential")
        || file_name.contains("secret")
}

enum DiffInspection {
    Text {
        text: String,
        truncated: bool,
        omitted_lines: u32,
    },
    Binary,
    Unavailable,
}

async fn diff_for_path(toplevel: &Path, git_path: &str) -> DiffInspection {
    match git_output(
        toplevel,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--numstat",
            "HEAD",
            "--",
            git_path,
        ],
    )
    .await
    {
        Ok(output) if output.status_success => {
            if is_binary_numstat(&output.stdout) {
                return DiffInspection::Binary;
            }
        }
        Ok(_) | Err(_) => return DiffInspection::Unavailable,
    }

    match git_output(
        toplevel,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-U3",
            "HEAD",
            "--",
            git_path,
        ],
    )
    .await
    {
        Ok(output) if output.status_success => {
            if output.stdout_bytes.contains(&0) || output.stdout.contains("Binary files ") {
                return DiffInspection::Binary;
            }
            let (text, truncated, omitted_lines) = bound_diff_lines(&output.stdout);
            DiffInspection::Text {
                text,
                truncated,
                omitted_lines,
            }
        }
        _ => DiffInspection::Unavailable,
    }
}

fn is_binary_numstat(stdout: &str) -> bool {
    stdout.lines().any(|line| {
        let mut parts = line.split('\t');
        matches!(
            (parts.next(), parts.next()),
            (Some("-"), Some("-")) | (Some("Bin"), _)
        )
    })
}

fn bound_diff_lines(diff: &str) -> (String, bool, u32) {
    let mut lines = Vec::new();
    let mut omitted_lines = 0_u32;
    let mut truncated = false;
    for line in diff.lines() {
        if lines.len() >= MAX_DIFF_LINES_PER_FILE {
            omitted_lines = omitted_lines.saturating_add(1);
            truncated = true;
            continue;
        }
        lines.push(line);
    }
    let joined = lines.join("\n");
    let (bounded, budget_truncated) = bound_diff_text(&joined, MAX_DIFF_BYTES_PER_FILE);
    (bounded, truncated || budget_truncated, omitted_lines)
}

fn bound_diff_text(text: &str, maximum_bytes: usize) -> (String, bool) {
    if text.len() <= maximum_bytes {
        return (text.to_owned(), false);
    }
    let mut end = maximum_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn git_in(path: &Path, args: &[&str]) {
        let status = StdCommand::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", GIT_NULL_CONFIG)
            .env("GIT_TERMINAL_PROMPT", "0")
            .status()
            .expect("git should spawn");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(root: &Path) {
        git_in(root, &["init", "-b", "main"]);
        git_in(root, &["config", "user.email", "fixture@example.test"]);
        git_in(root, &["config", "user.name", "Fixture"]);
        git_in(root, &["config", "commit.gpgsign", "false"]);
    }

    #[tokio::test]
    async fn a_non_repository_is_reported_without_git_write_commands() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace");

        let changes = inspect_workspace_changes(&workspace)
            .await
            .expect("inspection should succeed");
        assert_eq!(changes.kind, WorkspaceChangeKindDto::NotARepository);
        assert!(!changes.attributable_to_session);
        assert!(changes.entries.is_empty());
    }

    #[tokio::test]
    async fn dirty_and_untracked_paths_stay_relative_and_bounded() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace");
        init_repo(&workspace);
        std::fs::write(workspace.join("tracked.txt"), "one\n").expect("tracked file");
        git_in(&workspace, &["add", "tracked.txt"]);
        git_in(&workspace, &["commit", "-m", "fixture"]);
        std::fs::write(workspace.join("tracked.txt"), "two\n").expect("dirty tracked file");
        std::fs::write(workspace.join("notes.md"), "untracked\n").expect("untracked file");
        std::fs::write(workspace.join(".env"), "SECRET=private\n").expect("sensitive file");

        let changes = inspect_workspace_changes(&workspace)
            .await
            .expect("inspection should succeed");
        assert_eq!(changes.kind, WorkspaceChangeKindDto::Repository);
        assert!(!changes.attributable_to_session);
        let tracked = changes
            .entries
            .iter()
            .find(|entry| entry.path == "tracked.txt")
            .expect("tracked change");
        assert_eq!(tracked.status, WorkspaceChangeStatusDto::Modified);
        assert_eq!(tracked.content, WorkspaceChangeContentDto::Text);
        let diff = tracked.diff.as_deref().expect("text diff");
        assert!(diff.contains("two"));
        assert!(!diff.contains(workspace.to_str().unwrap()));

        let untracked = changes
            .entries
            .iter()
            .find(|entry| entry.path == "notes.md")
            .expect("untracked change");
        assert_eq!(untracked.status, WorkspaceChangeStatusDto::Untracked);
        assert_eq!(untracked.content, WorkspaceChangeContentDto::Omitted);
        assert_eq!(untracked.diff, None);

        let secret = changes
            .entries
            .iter()
            .find(|entry| entry.path == ".env")
            .expect("sensitive change");
        assert_eq!(secret.diff, None);
        assert!(!changes.entries.iter().any(|entry| {
            entry
                .diff
                .as_deref()
                .is_some_and(|diff| diff.contains("SECRET=private"))
        }));
    }

    #[tokio::test]
    async fn binary_files_and_escape_paths_fail_closed() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace");
        init_repo(&workspace);
        std::fs::write(workspace.join("data.bin"), [0_u8, 1, 2, 0, 3]).expect("binary");
        git_in(&workspace, &["add", "data.bin"]);
        git_in(&workspace, &["commit", "-m", "binary"]);
        std::fs::write(workspace.join("data.bin"), [0_u8, 9, 9, 0, 9]).expect("dirty binary");

        let changes = inspect_workspace_changes(&workspace)
            .await
            .expect("inspection should succeed");
        let binary = changes
            .entries
            .iter()
            .find(|entry| entry.path == "data.bin")
            .expect("binary change");
        assert_eq!(binary.content, WorkspaceChangeContentDto::Binary);
        assert_eq!(binary.diff, None);

        assert_eq!(normalize_git_path("../secret"), None);
        assert_eq!(normalize_git_path("/etc/passwd"), None);
        assert_eq!(
            display_path_in_workspace(&workspace, &workspace, "../outside.txt"),
            None
        );
    }

    #[tokio::test]
    async fn changes_outside_the_selected_workspace_are_omitted() {
        let root = tempfile::tempdir().expect("temporary root");
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).expect("repo");
        init_repo(&repo);
        std::fs::create_dir(repo.join("keep")).expect("selected folder");
        std::fs::create_dir(repo.join("other")).expect("other folder");
        std::fs::write(repo.join("keep/inside.txt"), "keep\n").expect("inside");
        std::fs::write(repo.join("other/outside.txt"), "other\n").expect("outside");
        git_in(&repo, &["add", "keep/inside.txt", "other/outside.txt"]);
        git_in(&repo, &["commit", "-m", "fixture"]);
        std::fs::write(repo.join("keep/inside.txt"), "changed\n").expect("dirty inside");
        std::fs::write(repo.join("other/outside.txt"), "changed\n").expect("dirty outside");

        let changes = inspect_workspace_changes(&repo.join("keep"))
            .await
            .expect("inspection should succeed");
        assert!(
            changes
                .entries
                .iter()
                .any(|entry| entry.path == "inside.txt")
        );
        assert!(
            !changes
                .entries
                .iter()
                .any(|entry| entry.path.contains("outside"))
        );
    }

    #[tokio::test]
    async fn oversized_diffs_are_truncated() {
        let root = tempfile::tempdir().expect("temporary root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace");
        init_repo(&workspace);
        std::fs::write(workspace.join("big.txt"), "base\n").expect("base");
        git_in(&workspace, &["add", "big.txt"]);
        git_in(&workspace, &["commit", "-m", "fixture"]);
        let dirty = (0..400)
            .map(|index| format!("line-{index}-{}", "x".repeat(80)))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(workspace.join("big.txt"), dirty).expect("huge dirty file");

        let changes = inspect_workspace_changes(&workspace)
            .await
            .expect("inspection should succeed");
        let big = changes
            .entries
            .iter()
            .find(|entry| entry.path == "big.txt")
            .expect("truncated change");
        assert!(big.truncated);
        assert!(changes.omitted_line_count > 0 || big.truncated);
        let diff = big.diff.as_deref().expect("bounded diff");
        assert!(diff.len() <= MAX_DIFF_BYTES_PER_FILE);
    }

    #[tokio::test]
    async fn missing_or_non_directory_paths_are_rejected() {
        let root = tempfile::tempdir().expect("temporary root");
        let missing = root.path().join("missing");
        let error = inspect_workspace_changes(&missing)
            .await
            .expect_err("missing workspace should fail");
        assert_eq!(
            error.code,
            crate::application_contract::ApplicationErrorCodeDto::InvalidWorkspace
        );

        let file = root.path().join("file");
        std::fs::write(&file, b"no").expect("file");
        let error = inspect_workspace_changes(&file)
            .await
            .expect_err("file workspace should fail");
        assert_eq!(
            error.code,
            crate::application_contract::ApplicationErrorCodeDto::InvalidWorkspace
        );
    }

    #[test]
    fn read_only_git_argument_lists_never_include_write_verbs() {
        for argument in READ_ONLY_STATUS_ARGS {
            assert!(!matches!(
                *argument,
                "add" | "commit" | "checkout" | "reset" | "clean" | "stash" | "push" | "rm"
            ));
        }
        let production = include_str!("workspace_changes.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source");
        for verb in [
            "add", "commit", "checkout", "reset", "clean", "stash", "push",
        ] {
            assert!(
                !production.contains(&format!("\"{verb}\"")),
                "workspace inspection must not invoke git {verb}"
            );
        }
    }

    #[test]
    fn porcelain_rename_records_preserve_both_relative_paths() {
        let mut encoded = b"R  ".to_vec();
        encoded.extend_from_slice(b"new.txt\0old.txt\0");
        let parsed = parse_porcelain_status(&encoded);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].status, WorkspaceChangeStatusDto::Renamed);
        assert_eq!(parsed.entries[0].path, "new.txt");
        assert_eq!(parsed.entries[0].previous_path.as_deref(), Some("old.txt"));
    }
}
