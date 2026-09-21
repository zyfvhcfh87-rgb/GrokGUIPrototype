use std::{
    collections::HashMap,
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

const READ_ONLY_STATUS_ARGS: &[&str] = &["status", "--porcelain=v1", "-z", "--untracked-files=all"];

pub async fn inspect_workspace_changes(
    path: &Path,
) -> Result<WorkspaceChangesDto, ApplicationErrorDto> {
    let workspace = canonicalize_workspace(path).map_err(ApplicationErrorDto::from)?;
    inspect_canonical_workspace(&workspace).await
}

async fn inspect_canonical_workspace(
    workspace: &Path,
) -> Result<WorkspaceChangesDto, ApplicationErrorDto> {
    let inside = match git_output(workspace, ["rev-parse", "--is-inside-work-tree"], false).await {
        Ok((output, _)) if output.status_success && output.stdout.trim() == "true" => true,
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

    let toplevel = match git_output(workspace, ["rev-parse", "--show-toplevel"], false).await {
        Ok((output, _)) if output.status_success => {
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

    let status = match git_output(&toplevel, READ_ONLY_STATUS_ARGS, false).await {
        Ok((output, _)) if output.status_success => output.stdout_bytes,
        Ok(_) | Err(_) => return Ok(WorkspaceChangesDto::unavailable()),
    };

    let parsed = parse_porcelain_status(&status);
    let mut omitted_entry_count = 0_u32;
    let mut omitted_line_count = 0_u32;
    let mut truncated = parsed.truncated;
    let mut total_diff_bytes = 0_usize;
    let mut entries = Vec::new();
    let mut diff_paths = Vec::new();

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

        let entry = WorkspaceChangeEntryDto {
            path: relative,
            previous_path,
            status: record.status,
            content: WorkspaceChangeContentDto::Omitted,
            diff: None,
            truncated: false,
        };

        if !should_omit_diff(&entry.path, entry.status) {
            diff_paths.push(record.path);
        }
        entries.push(entry);
    }

    let diffs = diffs_for_paths(&toplevel, &diff_paths).await;
    if diffs.output_truncated {
        truncated = true;
    }
    let mut diff_index = 0_usize;
    for entry in &mut entries {
        if should_omit_diff(&entry.path, entry.status) {
            continue;
        }
        let git_path = diff_paths.get(diff_index).map(String::as_str).unwrap_or("");
        diff_index += 1;
        let inspection =
            diffs
                .by_path
                .get(git_path)
                .cloned()
                .unwrap_or(if diffs.output_truncated {
                    DiffInspection::Text {
                        text: String::new(),
                        truncated: true,
                        omitted_lines: 0,
                    }
                } else {
                    DiffInspection::Unavailable
                });
        match inspection {
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

async fn git_output(
    workspace: &Path,
    args: impl IntoIterator<Item = impl AsRef<str>>,
    truncate_output: bool,
) -> Result<(GitOutput, bool), GitInvokeError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(workspace);
    command.arg("--no-pager");
    command.arg("--no-optional-locks");
    for arg in args {
        command.arg(arg.as_ref());
    }
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

    let output_truncated = output.stdout.len() > MAX_GIT_OUTPUT_BYTES;
    if output_truncated && !truncate_output {
        return Err(GitInvokeError::Failed);
    }
    let stdout_bytes = if output_truncated {
        output.stdout[..MAX_GIT_OUTPUT_BYTES].to_vec()
    } else {
        output.stdout
    };

    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let _ = RedactedDiagnostic::new(String::from_utf8_lossy(&output.stderr));
    Ok((
        GitOutput {
            status_success: output.status.success(),
            stdout,
            stdout_bytes,
        },
        output_truncated,
    ))
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

#[derive(Clone)]
enum DiffInspection {
    Text {
        text: String,
        truncated: bool,
        omitted_lines: u32,
    },
    Binary,
    Unavailable,
}

struct BatchedDiffs {
    by_path: HashMap<String, DiffInspection>,
    output_truncated: bool,
}

async fn diffs_for_paths(toplevel: &Path, paths: &[String]) -> BatchedDiffs {
    if paths.is_empty() {
        return BatchedDiffs {
            by_path: HashMap::new(),
            output_truncated: false,
        };
    }

    let mut numstat_args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--numstat",
        "-z",
        "HEAD",
        "--",
    ];
    for path in paths {
        numstat_args.push(path);
    }
    let mut diff_args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "-U3",
        "HEAD",
        "--",
    ];
    for path in paths {
        diff_args.push(path);
    }

    let numstat = git_output(toplevel, numstat_args, true).await;
    let unified = git_output(toplevel, diff_args, true).await;
    let output_truncated = matches!(&numstat, Ok((_, truncated)) if *truncated)
        || matches!(&unified, Ok((_, truncated)) if *truncated);
    let numstat_ok = matches!(&numstat, Ok((output, _)) if output.status_success);
    let unified_ok = matches!(&unified, Ok((output, _)) if output.status_success);
    let mut by_path = HashMap::new();

    if !numstat_ok && !unified_ok {
        for path in paths {
            by_path.insert(path.clone(), DiffInspection::Unavailable);
        }
        return BatchedDiffs {
            by_path,
            output_truncated,
        };
    }

    if let Ok((output, _)) = &unified
        && output.status_success
    {
        for (path, section) in split_unified_diff(&output.stdout_bytes) {
            by_path.insert(path, inspection_from_section(&section));
        }
    }

    if let Ok((output, _)) = &numstat
        && output.status_success
    {
        for (path, binary) in parse_numstat_z(&output.stdout_bytes) {
            if binary {
                by_path.insert(path, DiffInspection::Binary);
            } else if unified_ok && !output_truncated {
                by_path
                    .entry(path)
                    .or_insert_with(DiffInspection::empty_text);
            }
        }
    }

    BatchedDiffs {
        by_path,
        output_truncated,
    }
}

impl DiffInspection {
    fn empty_text() -> Self {
        Self::Text {
            text: String::new(),
            truncated: false,
            omitted_lines: 0,
        }
    }
}

fn inspection_from_section(section: &[u8]) -> DiffInspection {
    let text = String::from_utf8_lossy(section);
    if section.contains(&0) || text.contains("Binary files ") {
        return DiffInspection::Binary;
    }
    let (text, truncated, omitted_lines) = bound_diff_lines(&text);
    DiffInspection::Text {
        text,
        truncated,
        omitted_lines,
    }
}

fn parse_numstat_z(bytes: &[u8]) -> Vec<(String, bool)> {
    let mut entries = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(field) = read_raw_field(bytes, &mut cursor) else {
            break;
        };
        let Some((binary, mut path)) = parse_numstat_field(field) else {
            continue;
        };
        if cursor < bytes.len() && !field_starts_with_stat(bytes, cursor) {
            if let Some(destination) = read_raw_field(bytes, &mut cursor)
                && let Ok(destination) = std::str::from_utf8(destination)
                && !destination.is_empty()
            {
                path = destination.to_owned();
            }
        }
        entries.push((path, binary));
    }
    entries
}

fn field_starts_with_stat(bytes: &[u8], cursor: usize) -> bool {
    let rest = &bytes[cursor..];
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(rest.len());
    rest[..end].contains(&b'\t')
}

fn read_raw_field<'a>(bytes: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    if *cursor >= bytes.len() {
        return None;
    }
    let start = *cursor;
    while *cursor < bytes.len() && bytes[*cursor] != 0 {
        *cursor += 1;
    }
    if *cursor >= bytes.len() {
        return None;
    }
    let field = &bytes[start..*cursor];
    *cursor += 1;
    Some(field)
}

fn parse_numstat_field(field: &[u8]) -> Option<(bool, String)> {
    let text = std::str::from_utf8(field).ok()?;
    let mut parts = text.split('\t');
    let added = parts.next()?;
    let removed = parts.next()?;
    let path = parts.next()?;
    if path.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((added == "-" && removed == "-", path.to_owned()))
}

fn split_unified_diff(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let marker = b"diff --git ";
    let mut starts = Vec::new();
    let mut index = 0;
    while index + marker.len() <= bytes.len() {
        let at_line = index == 0 || bytes[index - 1] == b'\n';
        if at_line && bytes[index..].starts_with(marker) {
            starts.push(index);
            index += marker.len();
        } else {
            index += 1;
        }
    }
    let mut sections = Vec::new();
    for (offset, start) in starts.iter().copied().enumerate() {
        let end = starts.get(offset + 1).copied().unwrap_or(bytes.len());
        let section = &bytes[start..end];
        let header_end = section
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(section.len());
        let header = String::from_utf8_lossy(&section[..header_end]);
        if let Some(path) = destination_from_diff_header(&header) {
            sections.push((path, section.to_vec()));
        }
    }
    sections
}

fn destination_from_diff_header(header: &str) -> Option<String> {
    let rest = header
        .trim_end_matches(['\r', '\n'])
        .strip_prefix("diff --git ")?;
    let destination = if rest.starts_with('"') {
        let (_, after_source) = take_git_quoted(rest)?;
        let (destination, _) = take_git_quoted(after_source.trim_start())?;
        unquote_git_path(destination)
    } else {
        let marker = rest.rfind(" b/")?;
        rest[marker + 3..].to_owned()
    };
    let destination = destination.replace('\\', "/");
    if destination.is_empty() || destination == "/dev/null" {
        return None;
    }
    Some(destination)
}

fn take_git_quoted(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('"')?;
    let mut escaped = false;
    for (index, character) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == '"' {
            return Some((&rest[..index], &rest[index + 1..]));
        }
    }
    None
}

fn unquote_git_path(quoted: &str) -> String {
    let mut path = String::new();
    let mut chars = quoted.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            path.push(character);
            continue;
        }
        match chars.next() {
            Some('\\') => path.push('\\'),
            Some('"') => path.push('"'),
            Some('n') => path.push('\n'),
            Some('t') => path.push('\t'),
            Some(digit) if digit.is_ascii_digit() => {
                let mut octal = String::new();
                octal.push(digit);
                for _ in 0..2 {
                    if chars.peek().is_some_and(|next| next.is_ascii_digit()) {
                        octal.push(chars.next().expect("digit is present"));
                    }
                }
                if let Ok(value) = u8::from_str_radix(&octal, 8) {
                    path.push(value as char);
                }
            }
            Some(other) => path.push(other),
            None => path.push('\\'),
        }
    }
    path.strip_prefix("b/").unwrap_or(&path).to_owned()
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
    use std::{process::Command as StdCommand, sync::OnceLock};

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

    fn shared_workspace() -> &'static Path {
        static WORKSPACE: OnceLock<PathBuf> = OnceLock::new();
        WORKSPACE.get_or_init(|| {
            let root = tempfile::tempdir().expect("temporary root");
            let workspace = root.path().join("workspace");
            std::fs::create_dir(&workspace).expect("workspace");
            init_repo(&workspace);
            std::fs::write(workspace.join(".gitignore"), "ignored.txt\nnode_modules/\n")
                .expect("gitignore");
            std::fs::write(workspace.join("tracked.txt"), "one\n").expect("tracked file");
            std::fs::write(workspace.join("data.bin"), [0_u8, 1, 2, 0, 3]).expect("binary");
            std::fs::create_dir(workspace.join("keep")).expect("selected folder");
            std::fs::create_dir(workspace.join("other")).expect("other folder");
            std::fs::write(workspace.join("keep/inside.txt"), "keep\n").expect("inside");
            std::fs::write(workspace.join("other/outside.txt"), "other\n").expect("outside");
            std::fs::write(workspace.join("big.txt"), "base\n").expect("base");
            git_in(
                &workspace,
                &[
                    "add",
                    ".gitignore",
                    "tracked.txt",
                    "data.bin",
                    "keep/inside.txt",
                    "other/outside.txt",
                    "big.txt",
                ],
            );
            git_in(&workspace, &["commit", "-m", "fixture"]);
            std::fs::write(workspace.join("tracked.txt"), "two\n").expect("dirty tracked file");
            std::fs::write(workspace.join("notes.md"), "untracked\n").expect("untracked file");
            std::fs::write(workspace.join(".env"), "SECRET=private\n").expect("sensitive file");
            std::fs::write(workspace.join("data.bin"), [0_u8, 9, 9, 0, 9]).expect("dirty binary");
            std::fs::write(workspace.join("keep/inside.txt"), "changed\n").expect("dirty inside");
            std::fs::write(workspace.join("other/outside.txt"), "changed\n")
                .expect("dirty outside");
            std::fs::write(workspace.join("ignored.txt"), "ignored\n").expect("ignored file");
            std::fs::create_dir(workspace.join("node_modules")).expect("ignored directory");
            std::fs::write(workspace.join("node_modules/pkg.js"), "x\n").expect("ignored package");
            let dirty = (0..400)
                .map(|index| format!("line-{index}-{}", "x".repeat(80)))
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(workspace.join("big.txt"), dirty).expect("huge dirty file");
            let path = workspace.clone();
            std::mem::forget(root);
            path
        })
    }

    #[tokio::test]
    async fn dirty_and_untracked_paths_stay_relative_and_bounded() {
        let workspace = shared_workspace();

        let changes = inspect_workspace_changes(workspace)
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
        assert!(
            !changes
                .entries
                .iter()
                .any(|entry| entry.path == "ignored.txt" || entry.path.contains("node_modules"))
        );
    }

    #[tokio::test]
    async fn binary_files_and_escape_paths_fail_closed() {
        let workspace = shared_workspace();

        let changes = inspect_workspace_changes(workspace)
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
            display_path_in_workspace(workspace, workspace, "../outside.txt"),
            None
        );
    }

    #[tokio::test]
    async fn changes_outside_the_selected_workspace_are_omitted() {
        let changes = inspect_workspace_changes(&shared_workspace().join("keep"))
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
        let changes = inspect_workspace_changes(shared_workspace())
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
        assert!(
            !production.contains("--ignored"),
            "workspace inspection must not enumerate ignored paths"
        );
    }

    #[test]
    fn numstat_z_and_unified_headers_keep_destination_paths() {
        let numstat = b"-\t-\tdata.bin\01\t0\told.txt\0new.txt\01\t1\ttracked.txt\0";
        let parsed = parse_numstat_z(numstat);
        assert_eq!(
            parsed,
            vec![
                ("data.bin".to_owned(), true),
                ("new.txt".to_owned(), false),
                ("tracked.txt".to_owned(), false),
            ]
        );

        let diff = b"diff --git a/keep/inside.txt b/keep/inside.txt\n+changed\ndiff --git a/tracked.txt b/tracked.txt\n+two\n";
        let sections = split_unified_diff(diff);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, "keep/inside.txt");
        assert_eq!(sections[1].0, "tracked.txt");
        assert!(matches!(
            inspection_from_section(&sections[1].1),
            DiffInspection::Text { .. }
        ));
        assert!(matches!(
            inspection_from_section(b"diff --git a/data.bin b/data.bin\nBinary files a/data.bin and b/data.bin differ\n"),
            DiffInspection::Binary
        ));
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
