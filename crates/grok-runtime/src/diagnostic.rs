use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

const REDACTED: &str = "[REDACTED]";
const REDACTED_PATH: &str = "[PATH]";
const TRUNCATED: &str = " [TRUNCATED]";

// Diagnostics are presentation data, not a lossless log transport. Bounding
// the raw prefix keeps every sanitizer pass predictable even when an upstream
// error contains an attacker-controlled or accidentally enormous payload.
const MAX_DIAGNOSTIC_INPUT_BYTES: usize = 16 * 1024;
const MAX_DIAGNOSTIC_OUTPUT_BYTES: usize = 4 * 1024;

const SECRET_LABELS: &[SecretLabel] = &[
    SecretLabel::new("proxy-authorization", true),
    SecretLabel::new("refresh_token", false),
    SecretLabel::new("refresh-token", false),
    SecretLabel::new("refreshToken", false),
    SecretLabel::new("access_token", false),
    SecretLabel::new("access-token", false),
    SecretLabel::new("accessToken", false),
    SecretLabel::new("client_secret", false),
    SecretLabel::new("client-secret", false),
    SecretLabel::new("clientSecret", false),
    SecretLabel::new("proxyAuthorization", true),
    SecretLabel::new("authorization", true),
    SecretLabel::new("x-api-key", false),
    SecretLabel::new("xApiKey", false),
    SecretLabel::new("auth_token", false),
    SecretLabel::new("auth-token", false),
    SecretLabel::new("authToken", false),
    SecretLabel::new("api_key", false),
    SecretLabel::new("api-key", false),
    SecretLabel::new("api key", false),
    SecretLabel::new("apikey", false),
    SecretLabel::new("password", false),
    SecretLabel::new("passwd", false),
    SecretLabel::new("credential", false),
    SecretLabel::new("cookie", false),
    SecretLabel::new("secret", false),
    SecretLabel::new("token", false),
];

#[derive(Clone, Copy)]
struct SecretLabel {
    text: &'static str,
    authorization: bool,
}

impl SecretLabel {
    const fn new(text: &'static str, authorization: bool) -> Self {
        Self {
            text,
            authorization,
        }
    }
}

/// Text that has passed through the runtime's diagnostic sanitizer.
///
/// The inner string is intentionally private. Construction and deserialization
/// both redact it, so normalized failure events cannot accidentally bypass the
/// sanitizer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactedDiagnostic(String);

impl RedactedDiagnostic {
    pub fn new(raw: impl AsRef<str>) -> Self {
        Self(redact_diagnostic(raw.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for RedactedDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for RedactedDiagnostic {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RedactedDiagnostic {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer).map_err(D::Error::custom)?;
        Ok(Self::new(raw))
    }
}

/// Bound and sanitize text before it is displayed or persisted as diagnostic
/// data. Common credentials, absolute paths, and display-control characters
/// are removed without discarding useful relative technical context.
pub fn redact_diagnostic(input: &str) -> String {
    redact_with_home(input)
}

fn redact_with_home(input: &str) -> String {
    let (bounded, input_was_truncated) = bounded_prefix(input, MAX_DIAGNOSTIC_INPUT_BYTES);
    let mut redacted = normalize_controls(bounded);

    // Remove whole locations before replacing values inside them. Otherwise a
    // marker such as `[REDACTED]` can introduce punctuation that prematurely
    // terminates the later URL/path scan and leaves a private suffix behind.
    redacted = redact_urls(&redacted);
    redacted = redact_absolute_paths(&redacted);
    redacted = redact_sensitive_assignment_keys(&redacted);

    for label in SECRET_LABELS {
        redacted = redact_secret_label(&redacted, *label);
    }

    redacted = redact_bearer_tokens(&redacted);
    redacted = redact_long_key_prefixes(&redacted, input_was_truncated);

    bound_sanitized_output(redacted, input_was_truncated)
}

fn bounded_prefix(input: &str, maximum_bytes: usize) -> (&str, bool) {
    if input.len() <= maximum_bytes {
        return (input, false);
    }

    let mut end = maximum_bytes;
    while !input.is_char_boundary(end) {
        end -= 1;
    }

    (&input[..end], true)
}

fn normalize_controls(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut characters = input.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                output.push('\n');
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
            }
            '\n' => output.push('\n'),
            character if is_display_control(character) => output.push(' '),
            character => output.push(character),
        }
    }

    output
}

fn is_display_control(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
        )
}

fn bound_sanitized_output(mut output: String, input_was_truncated: bool) -> String {
    let output_was_truncated = output.len() > MAX_DIAGNOSTIC_OUTPUT_BYTES;
    if !input_was_truncated && !output_was_truncated {
        return output;
    }

    let prefix_budget = MAX_DIAGNOSTIC_OUTPUT_BYTES.saturating_sub(TRUNCATED.len());
    if output.len() > prefix_budget {
        let mut end = prefix_budget;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        end = avoid_cutting_sanitizer_marker(&output, end);
        output.truncate(end);
    }

    if !output.ends_with(TRUNCATED) {
        output.push_str(TRUNCATED);
    }
    output
}

fn avoid_cutting_sanitizer_marker(input: &str, end: usize) -> usize {
    let Some(marker_start) = input[..end].rfind('[') else {
        return end;
    };

    let crosses_boundary = [REDACTED, REDACTED_PATH, TRUNCATED.trim_start()]
        .iter()
        .any(|marker| {
            input[marker_start..].starts_with(marker) && marker_start + marker.len() > end
        });

    if crosses_boundary { marker_start } else { end }
}

fn redact_secret_label(input: &str, label: SecretLabel) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut search_from = 0;

    while let Some(label_start) = find_text(input, label.text, search_from, true) {
        let label_end = label_start + label.text.len();
        if !label_match_has_boundaries(input, label_start, label_end) {
            search_from = next_char_boundary(input, label_start);
            continue;
        }

        let Some((value_start, value_end)) =
            secret_value_range(input, label_end, label.authorization)
        else {
            search_from = label_end;
            continue;
        };

        output.push_str(&input[copied_through..value_start]);
        output.push_str(REDACTED);
        copied_through = value_end;
        search_from = value_end;
    }

    output.push_str(&input[copied_through..]);
    output
}

fn redact_sensitive_assignment_keys(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;

    for (delimiter_index, delimiter) in input.char_indices() {
        if !matches!(delimiter, ':' | '=') || delimiter_index < copied_through {
            continue;
        }
        let Some((key_start, key_end)) = assignment_key_before(input, delimiter_index) else {
            continue;
        };
        let key = &input[key_start..key_end];
        let Some(authorization) = sensitive_assignment_key(key) else {
            continue;
        };
        let Some((value_start, value_end)) = secret_value_range(input, key_end, authorization)
        else {
            continue;
        };
        if value_start < copied_through {
            continue;
        }

        output.push_str(&input[copied_through..value_start]);
        output.push_str(REDACTED);
        copied_through = value_end;
    }

    output.push_str(&input[copied_through..]);
    output
}

fn assignment_key_before(input: &str, delimiter_index: usize) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    let mut end = delimiter_index;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end > 0 && matches!(bytes[end - 1], b'\'' | b'"') {
        end -= 1;
    }

    let mut start = end;
    while start > 0 && is_assignment_key_byte(bytes[start - 1]) {
        start -= 1;
    }
    (start < end).then_some((start, end))
}

fn is_assignment_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

/// Returns whether values for this complete assignment key need
/// Authorization-style scheme handling. Compound keys are compared after
/// separator removal so common env, JSON, and camelCase spellings converge.
fn sensitive_assignment_key(key: &str) -> Option<bool> {
    const EXACT: &[&str] = &[
        "token",
        "secret",
        "password",
        "passwd",
        "credential",
        "cookie",
        "authorization",
        "apikey",
        "xapikey",
    ];
    const COMPOUNDS: &[&str] = &[
        "accesstoken",
        "refreshtoken",
        "authtoken",
        "sessiontoken",
        "securitytoken",
        "bearertoken",
        "clientsecret",
        "secretaccesskey",
        "privatekey",
        "apikey",
        "password",
        "passwd",
        "credential",
        "cookie",
        "authorization",
    ];

    let compact: String = key
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect();
    let sensitive = EXACT.contains(&compact.as_str())
        || COMPOUNDS.iter().any(|needle| compact.contains(needle));
    sensitive.then(|| compact.contains("authorization"))
}

fn label_match_has_boundaries(input: &str, start: usize, end: usize) -> bool {
    let before_is_safe = input[..start]
        .chars()
        .next_back()
        .is_none_or(|character| !character.is_ascii_alphanumeric());
    let after_is_safe = input[end..]
        .chars()
        .next()
        .is_none_or(|character| !is_identifier_character(character));

    before_is_safe && after_is_safe
}

fn secret_value_range(
    input: &str,
    label_end: usize,
    authorization: bool,
) -> Option<(usize, usize)> {
    let mut cursor = skip_ascii_whitespace(input, label_end);

    if matches!(byte_at(input, cursor), Some(b'\'' | b'"')) {
        cursor += 1;
        cursor = skip_ascii_whitespace(input, cursor);
    }

    let Some(delimiter @ (b':' | b'=')) = byte_at(input, cursor) else {
        return None;
    };

    cursor += 1;
    cursor = skip_ascii_whitespace(input, cursor);
    if delimiter == b'=' && byte_at(input, cursor) == Some(b'>') {
        cursor += 1;
        cursor = skip_ascii_whitespace(input, cursor);
    }
    let quote = match byte_at(input, cursor) {
        Some(byte @ (b'\'' | b'"')) => {
            cursor += 1;
            Some(byte)
        }
        _ => None,
    };

    let value_start = cursor;
    let value_end = match quote {
        Some(quote) => find_closing_quote(input, value_start, quote).unwrap_or(input.len()),
        None if authorization => authorization_value_end(input, value_start),
        // Unquoted credential values may contain spaces. Keep this boundary
        // deliberately conservative so a passphrase or debug-map rendering
        // cannot leak its suffix after the first word.
        None => input[value_start..]
            .find('\n')
            .map_or(input.len(), |offset| value_start + offset),
    };

    (value_end > value_start).then_some((value_start, value_end))
}

fn authorization_value_end(input: &str, value_start: usize) -> usize {
    let first_end = unquoted_value_end(input, value_start);
    let scheme = &input[value_start..first_end];

    if ["bearer", "basic", "digest"]
        .iter()
        .any(|candidate| scheme.eq_ignore_ascii_case(candidate))
    {
        let credential_start = skip_ascii_whitespace(input, first_end);
        let credential_end = unquoted_value_end(input, credential_start);
        if credential_end > credential_start {
            return credential_end;
        }
    }

    first_end
}

fn redact_bearer_tokens(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut search_from = 0;

    while let Some(start) = find_text(input, "bearer", search_from, true) {
        let word_end = start + "bearer".len();
        let before_is_safe = input[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !is_identifier_character(character));
        let has_space = input[word_end..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace);

        if !before_is_safe || !has_space {
            search_from = next_char_boundary(input, start);
            continue;
        }

        let mut value_start = skip_ascii_whitespace(input, word_end);
        let quote = match byte_at(input, value_start) {
            Some(byte @ (b'\'' | b'"')) => {
                value_start += 1;
                Some(byte)
            }
            _ => None,
        };
        let value_end = quote
            .and_then(|quote| find_closing_quote(input, value_start, quote))
            .unwrap_or_else(|| unquoted_value_end(input, value_start));

        if value_end == value_start {
            search_from = word_end;
            continue;
        }

        output.push_str(&input[copied_through..value_start]);
        output.push_str(REDACTED);
        copied_through = value_end;
        search_from = value_end;
    }

    output.push_str(&input[copied_through..]);
    output
}

fn redact_long_key_prefixes(input: &str, input_was_truncated: bool) -> String {
    ["xai-", "sk-"]
        .into_iter()
        .fold(input.to_owned(), |text, prefix| {
            redact_long_key_prefix(&text, prefix, input_was_truncated)
        })
}

fn redact_long_key_prefix(input: &str, prefix: &str, input_was_truncated: bool) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut search_from = 0;

    while let Some(start) = find_text(input, prefix, search_from, true) {
        let before_is_safe = input[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !is_key_character(character));
        if !before_is_safe {
            search_from = next_char_boundary(input, start);
            continue;
        }

        let end = input[start..]
            .char_indices()
            .take_while(|(_, character)| is_key_character(*character))
            .last()
            .map_or(start, |(offset, character)| {
                start + offset + character.len_utf8()
            });

        let truncated_suffix = input_was_truncated && end == input.len();
        if end - start < prefix.len() + 16 && !truncated_suffix {
            search_from = next_char_boundary(input, start);
            continue;
        }

        output.push_str(&input[copied_through..start]);
        output.push_str(REDACTED);
        copied_through = end;
        search_from = end;
    }

    output.push_str(&input[copied_through..]);
    output
}

fn redact_absolute_paths(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut cursor = 0;

    while cursor < input.len() {
        if is_absolute_path_start(input, cursor) {
            let end = absolute_path_end(input, cursor);
            output.push_str(&input[copied_through..cursor]);
            output.push_str(REDACTED_PATH);
            copied_through = end;
            cursor = end;
        } else {
            cursor = next_char_boundary(input, cursor);
        }
    }

    output.push_str(&input[copied_through..]);
    output
}

fn redact_urls(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut cursor = 0;

    while cursor < input.len() {
        if is_url_start(input, cursor) {
            let end = url_end(input, cursor);
            output.push_str(&input[copied_through..cursor]);
            output.push_str(REDACTED_PATH);
            copied_through = end;
            cursor = end;
        } else {
            cursor = next_char_boundary(input, cursor);
        }
    }

    output.push_str(&input[copied_through..]);
    output
}

fn is_url_start(input: &str, start: usize) -> bool {
    if !input.is_char_boundary(start) || !path_start_has_boundary(input, start) {
        return false;
    }

    let bytes = input.as_bytes();
    if !bytes.get(start).is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }

    let maximum_scheme_end = (start + 32).min(bytes.len());
    let mut cursor = start + 1;
    while cursor < maximum_scheme_end {
        match bytes[cursor] {
            b':' => return bytes.get(cursor + 1..cursor + 3) == Some(b"//"),
            byte if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.') => {
                cursor += 1;
            }
            _ => return false,
        }
    }

    false
}

fn url_end(input: &str, start: usize) -> usize {
    let quote = input[..start]
        .chars()
        .next_back()
        .filter(|character| matches!(character, '\'' | '"' | '`'));

    for (offset, character) in input[start..].char_indices().skip(1) {
        let ends_url = match quote {
            Some(quote) => character == quote || character == '\n',
            None => {
                character.is_whitespace()
                    || matches!(character, '\'' | '"' | '`' | ')' | ']' | '}' | '>')
            }
        };
        if ends_url {
            return start + offset;
        }
    }

    input.len()
}

fn is_absolute_path_start(input: &str, start: usize) -> bool {
    if !input.is_char_boundary(start) || !path_start_has_boundary(input, start) {
        return false;
    }

    let remaining = &input[start..];
    // Serialized JSON doubles each native separator. Four leading slashes are
    // therefore the common rendered form of both UNC and extended paths.
    if remaining.starts_with("\\\\\\\\") {
        return true;
    }
    let is_home_relative = ["~/", "~\\"]
        .iter()
        .any(|prefix| remaining.starts_with(prefix));
    let is_home_environment_path = [
        "$HOME/",
        "$HOME\\",
        "${HOME}/",
        "${HOME}\\",
        "%USERPROFILE%/",
        "%USERPROFILE%\\",
        "%HOME%/",
        "%HOME%\\",
    ]
    .iter()
    .any(|prefix| {
        remaining
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
    });
    if is_home_relative || is_home_environment_path {
        return true;
    }

    let bytes = input.as_bytes();
    let drive_path = bytes.get(start).is_some_and(u8::is_ascii_alphabetic)
        && bytes.get(start + 1) == Some(&b':')
        && bytes
            .get(start + 2)
            .is_some_and(|byte| is_path_separator(*byte));
    if drive_path {
        return true;
    }

    let Some(first) = bytes.get(start).copied() else {
        return false;
    };
    if first == b'/' && bytes.get(start + 1).is_some_and(|byte| *byte != b'/') {
        return bytes
            .get(start + 1)
            .is_some_and(|byte| !byte.is_ascii_whitespace());
    }
    if first == b'\\' && bytes.get(start + 1).is_some_and(|byte| *byte != b'\\') {
        return bytes
            .get(start + 1)
            .is_some_and(|byte| !byte.is_ascii_whitespace());
    }

    is_unc_or_extended_path(input, start)
}

fn path_start_has_boundary(input: &str, start: usize) -> bool {
    input[..start]
        .chars()
        .next_back()
        .is_none_or(|character| !is_path_token_character(character))
}

fn is_path_token_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '/' | '\\')
}

fn is_unc_or_extended_path(input: &str, start: usize) -> bool {
    let bytes = input.as_bytes();
    if !bytes
        .get(start)
        .zip(bytes.get(start + 1))
        .is_some_and(|(first, second)| is_path_separator(*first) && is_path_separator(*second))
    {
        return false;
    }

    let component_start = start + 2;
    if matches!(bytes.get(component_start), Some(b'?' | b'.'))
        && bytes
            .get(component_start + 1)
            .is_some_and(|byte| is_path_separator(*byte))
    {
        return true;
    }

    let mut cursor = component_start;
    let mut component_has_content = false;
    while let Some(byte) = bytes.get(cursor).copied() {
        if is_path_terminator(byte as char, None) {
            return false;
        }
        if is_path_separator(byte) {
            return component_has_content
                && bytes
                    .get(cursor + 1)
                    .is_some_and(|next| !is_path_separator(*next) && !next.is_ascii_whitespace());
        }
        component_has_content = true;
        cursor += 1;
    }

    false
}

fn absolute_path_end(input: &str, start: usize) -> usize {
    let quote = input[..start]
        .chars()
        .next_back()
        .filter(|character| matches!(character, '\'' | '"' | '`'));

    for (offset, character) in input[start..].char_indices().skip(1) {
        if is_path_terminator(character, quote) {
            return start + offset;
        }
    }

    input.len()
}

fn is_path_terminator(character: char, quote: Option<char>) -> bool {
    match quote {
        Some(quote) => character == quote || character == '\n',
        // Unquoted native paths can legally contain whitespace and nearly all
        // punctuation. Consuming the rest of the line is intentionally
        // conservative: preserving prose is less important than never leaving
        // a private path suffix behind.
        None => character == '\n',
    }
}

fn is_path_separator(byte: u8) -> bool {
    matches!(byte, b'/' | b'\\')
}

fn find_text(input: &str, needle: &str, from: usize, case_insensitive: bool) -> Option<usize> {
    if needle.is_empty() || from > input.len() || !input.is_char_boundary(from) {
        return None;
    }

    input[from..].char_indices().find_map(|(offset, _)| {
        let start = from + offset;
        let end = start.checked_add(needle.len())?;
        let candidate = input.get(start..end)?;
        let matches = if case_insensitive {
            candidate.eq_ignore_ascii_case(needle)
        } else {
            candidate == needle
        };
        matches.then_some(start)
    })
}

fn skip_ascii_whitespace(input: &str, mut cursor: usize) -> usize {
    while byte_at(input, cursor).is_some_and(|byte| byte.is_ascii_whitespace()) {
        cursor += 1;
    }
    cursor
}

fn find_closing_quote(input: &str, value_start: usize, quote: u8) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut cursor = value_start;
    let mut escaped = false;

    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if byte == quote && !escaped {
            return Some(cursor);
        }

        escaped = byte == b'\\' && !escaped;
        if byte != b'\\' {
            escaped = false;
        }
        cursor += 1;
    }

    None
}

fn unquoted_value_end(input: &str, value_start: usize) -> usize {
    if input[value_start..].starts_with(REDACTED) {
        return value_start + REDACTED.len();
    }

    input[value_start..]
        .char_indices()
        .find(|(_, character)| is_unquoted_delimiter(*character))
        .map_or(input.len(), |(offset, _)| value_start + offset)
}

fn is_unquoted_delimiter(character: char) -> bool {
    character.is_whitespace() || matches!(character, ',' | ';' | '&' | '\'' | '"' | '}' | ']')
}

fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
}

fn is_key_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
}

fn byte_at(input: &str, index: usize) -> Option<u8> {
    input.as_bytes().get(index).copied()
}

fn next_char_boundary(input: &str, start: usize) -> usize {
    start + input[start..].chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_omits_sensitive_test_data(output: &str, sensitive_fragments: &[&str]) {
        assert!(
            sensitive_fragments
                .iter()
                .all(|fragment| !output.contains(fragment)),
            "sanitizer retained sensitive test data"
        );
    }

    #[test]
    fn redacts_bearer_and_common_api_key_forms() {
        let input = concat!(
            "Authorization: Bearer header-secret\n",
            "x-api-key = top-secret\n",
            "{\"apiKey\":\"json-secret\"}\n",
            "XAI_API_KEY=environment-secret\n",
            "standalone bearer loose-secret"
        );

        let redacted = redact_with_home(input);

        assert_omits_sensitive_test_data(
            &redacted,
            &[
                "header-secret",
                "top-secret",
                "json-secret",
                "environment-secret",
                "loose-secret",
            ],
        );
        assert_eq!(redacted.matches(REDACTED).count(), 5);
    }

    #[test]
    fn redacts_generic_sensitive_assignments_without_matching_technical_words() {
        let input = concat!(
            "password=private-passphrase\n",
            "token: private-session-token\n",
            "cookie=private-cookie-value\n",
            "credential: private-credential-value\n",
            "secret=private-secret-value\n",
            "password=correct horse battery staple\n",
            "token => private-debug-map-token\n",
            "token_count=42 and tokenizer=enabled"
        );

        let redacted = redact_with_home(input);

        assert_omits_sensitive_test_data(
            &redacted,
            &[
                "private-passphrase",
                "private-session-token",
                "private-cookie-value",
                "private-credential-value",
                "private-secret-value",
                "correct horse battery staple",
                "private-debug-map-token",
            ],
        );
        assert_eq!(redacted.matches(REDACTED).count(), 7);
        assert!(redacted.contains("token_count=42"));
        assert!(redacted.contains("tokenizer=enabled"));
    }

    #[test]
    fn redacts_common_camel_case_credential_assignments() {
        let input = concat!(
            r#"{"accessToken":"private-access","clientSecret":"private-client","refreshToken":"private-refresh"}"#,
            "\n",
            r#"{"authToken":"private-auth","proxyAuthorization":"Bearer private-proxy","xApiKey":"private-api-key"}"#
        );

        let redacted = redact_with_home(input);

        assert_omits_sensitive_test_data(
            &redacted,
            &[
                "private-access",
                "private-client",
                "private-refresh",
                "private-auth",
                "private-proxy",
                "private-api-key",
            ],
        );
        assert_eq!(redacted.matches(REDACTED).count(), 6);
    }

    #[test]
    fn redacts_compound_env_json_and_camel_case_credential_keys() {
        let input = concat!(
            "AWS_SECRET_ACCESS_KEY=private-aws\n",
            r#"{"AWS_SECRET_ACCESS_KEY":"private-json-aws"}"#,
            "\n",
            "SSH_PRIVATE_KEY=private-ssh\n",
            r#"{"privateKey":"private-json-key","sessionToken":"private-session"}"#,
            "\n",
            "token_count=42 and tokenizer=enabled"
        );

        let redacted = redact_with_home(input);

        assert_omits_sensitive_test_data(
            &redacted,
            &[
                "private-aws",
                "private-json-aws",
                "private-ssh",
                "private-json-key",
                "private-session",
            ],
        );
        assert_eq!(redacted.matches(REDACTED).count(), 5);
        assert!(redacted.contains("token_count=42"));
        assert!(redacted.contains("tokenizer=enabled"));
    }

    #[test]
    fn redacts_long_provider_key_prefixes_but_not_short_technical_names() {
        let input = format!(
            "key {} and package sk-learn",
            concat!("xai", "-1234567890abcdefghijkl")
        );

        let redacted = redact_with_home(&input);

        assert_eq!(redacted, "key [REDACTED] and package sk-learn");
    }

    #[test]
    fn redacts_absolute_paths_independent_of_the_current_home() {
        let input = concat!(
            r#"drive "D:\Other User\private\build.log""#,
            "\n",
            r#"unc \\server-name\share-name\folder\file"#,
            "\n",
            r#"extended \\?\C:\build-user\cache"#,
            "\n",
            r#"slash C:/Users/Ada/project"#,
            "\n",
            r#"unix /srv/build-user/work/item"#,
        );

        let redacted = RedactedDiagnostic::new(input);

        assert_omits_sensitive_test_data(
            redacted.as_str(),
            &[
                "Other User",
                "server-name",
                "share-name",
                "build-user",
                "Users/Ada",
            ],
        );
        assert_eq!(redacted.as_str().matches(REDACTED_PATH).count(), 5);
    }

    #[test]
    fn unquoted_absolute_paths_with_spaces_consume_the_private_line_tail() {
        let input = concat!(
            r#"drive D:\Private Project\secret.txt: denied"#,
            "\n",
            r#"unc \\server-name\Private Share\secret.txt: denied"#,
            "\n",
            "unix /srv/Private Project/secret.txt: denied",
        );

        let redacted = RedactedDiagnostic::new(input);

        assert_omits_sensitive_test_data(
            redacted.as_str(),
            &["Private Project", "Private Share", "secret.txt", "denied"],
        );
        assert_eq!(redacted.as_str().matches(REDACTED_PATH).count(), 3);
    }

    #[test]
    fn redacts_rooted_windows_paths_without_matching_relative_technical_paths() {
        let input = concat!(
            r#"rooted \Users\Example\private\trace.log"#,
            "\n",
            r#"relative crates\grok-runtime and prefix\Users\Example"#,
        );

        let redacted = RedactedDiagnostic::new(input);

        assert_omits_sensitive_test_data(redacted.as_str(), &[r"\Users\Example\private"]);
        assert!(redacted.as_str().contains(r"crates\grok-runtime"));
        assert!(redacted.as_str().contains(r"prefix\Users\Example"));
        assert_eq!(redacted.as_str().matches(REDACTED_PATH).count(), 1);
    }

    #[test]
    fn redacts_home_relative_and_home_environment_paths() {
        let input = concat!(
            "unix ~/.grok/auth.json\n",
            "windows ~\\.grok\\auth.json\n",
            "dollar $HOME/.grok/auth.json\n",
            "braced ${HOME}\\.grok\\auth.json\n",
            "profile %USERPROFILE%\\.grok\\auth.json\n",
            "percent %HOME%/.grok/auth.json"
        );

        let redacted = RedactedDiagnostic::new(input);

        assert_omits_sensitive_test_data(
            redacted.as_str(),
            &[".grok", "auth.json", "USERPROFILE", "${HOME}"],
        );
        assert_eq!(redacted.as_str().matches(REDACTED_PATH).count(), 6);
    }

    #[test]
    fn redacts_json_escaped_unc_and_extended_windows_paths() {
        let input = concat!(
            r#"payload={"cwd":"\\\\server\\share\\private\\session.json"}"#,
            "\n",
            r#"extended={"path":"\\\\?\\C:\\Users\\Private\\secret.txt"}"#
        );

        let redacted = RedactedDiagnostic::new(input);

        assert_omits_sensitive_test_data(
            redacted.as_str(),
            &["server", "share", "session.json", "Users", "secret.txt"],
        );
        assert_eq!(redacted.as_str().matches(REDACTED_PATH).count(), 2);
    }

    #[test]
    fn redacts_token_bearing_urls_and_file_uris() {
        let url_credential = ["url-", "credential-material"].concat();
        let input = format!(
            "request https://host.example?token={url_credential}#private-fragment denied\n\
             read file:///D:/Private/User/secret.txt failed"
        );

        let redacted = RedactedDiagnostic::new(&input);

        assert_omits_sensitive_test_data(
            redacted.as_str(),
            &[
                &url_credential,
                "host.example",
                "private-fragment",
                "Private/User",
                "secret.txt",
            ],
        );
        assert!(redacted.as_str().contains("request [PATH] denied"));
        assert!(redacted.as_str().contains("read [PATH] failed"));
        assert_eq!(redacted.as_str().matches(REDACTED_PATH).count(), 2);
    }

    #[test]
    fn punctuation_cannot_hide_absolute_paths_or_urls() {
        for punctuation in ['>', ')', '|', '!'] {
            let path = format!(r#"failed{punctuation}C:\Users\Private\secret.txt"#);
            let url = format!(
                "request{punctuation}https://host.example/?token=private-credential-material"
            );

            let redacted_path = RedactedDiagnostic::new(path);
            let redacted_url = RedactedDiagnostic::new(url);

            assert_omits_sensitive_test_data(redacted_path.as_str(), &["Private", "secret.txt"]);
            assert_omits_sensitive_test_data(
                redacted_url.as_str(),
                &["private-credential-material"],
            );
            assert!(redacted_path.as_str().contains(REDACTED_PATH));
            assert!(redacted_url.as_str().contains(REDACTED_PATH));
        }
    }

    #[test]
    fn preserves_useful_relative_technical_paths_and_methods() {
        let input = concat!(
            "module src/runtime/mod.rs; crate crates\\grok-runtime; ",
            "method x.ai/session-update; local ./fixtures/input.json"
        );

        assert_eq!(redact_with_home(input), input);
    }

    #[test]
    fn normalizes_display_controls_without_flattening_lines() {
        let input = "first\r\nsecond\tthird\u{1b}[31m\u{202e}fourth\0";

        let redacted = redact_with_home(input);

        assert!(redacted.contains("first\nsecond"));
        assert!(redacted.contains("third"));
        assert!(redacted.contains("fourth"));
        assert!(
            redacted
                .chars()
                .all(|character| character == '\n' || !is_display_control(character))
        );
    }

    #[test]
    fn deserialization_sanitizes_instead_of_bypassing_the_wrapper() {
        let diagnostic: RedactedDiagnostic =
            serde_json::from_str(r#""Bearer deserialize-secret""#).unwrap();

        assert_eq!(diagnostic.as_str(), "Bearer [REDACTED]");
        assert_eq!(
            serde_json::to_string(&diagnostic).unwrap(),
            r#""Bearer [REDACTED]""#
        );
    }

    #[test]
    fn secret_and_path_combinations_are_sanitized_before_serialization() {
        let provider_key = ["xai-", "0123456789abcdefghijklmnop"].concat();
        let bearer = ["session-", "credential-material"].concat();
        let path = [r"E:\Private User\", &provider_key, r"\trace.log"].concat();
        let raw = format!(r#"failed at "{path}"; Authorization: Bearer {bearer}"#);

        let diagnostic = RedactedDiagnostic::new(&raw);
        let serialized = serde_json::to_string(&diagnostic).unwrap();
        let round_trip: RedactedDiagnostic = serde_json::from_str(&serialized).unwrap();

        assert_omits_sensitive_test_data(
            diagnostic.as_str(),
            &[&provider_key, &bearer, "Private User"],
        );
        assert_eq!(diagnostic, round_trip);
        assert_eq!(serialized, serde_json::to_string(&round_trip).unwrap());
        assert!(diagnostic.as_str().len() <= MAX_DIAGNOSTIC_OUTPUT_BYTES);
    }

    #[test]
    fn huge_utf8_input_is_bounded_on_character_boundaries() {
        let mut input = "malformed frame ".to_owned();
        // Greek lambda is two bytes in UTF-8, so the byte cap can land inside
        // its encoding if boundary handling regresses.
        input.push_str(&"\u{03bb}".repeat(MAX_DIAGNOSTIC_INPUT_BYTES));
        input.push_str("Bearer trailing-sensitive-test-data");

        let encoded_raw = serde_json::to_string(&input).unwrap();
        let redacted: RedactedDiagnostic = serde_json::from_str(&encoded_raw).unwrap();

        assert!(redacted.as_str().len() <= MAX_DIAGNOSTIC_OUTPUT_BYTES);
        assert!(redacted.as_str().ends_with(TRUNCATED));
        assert!(redacted.as_str().is_char_boundary(redacted.as_str().len()));
        assert_omits_sensitive_test_data(redacted.as_str(), &["trailing-sensitive-test-data"]);

        let serialized = serde_json::to_string(&redacted).unwrap();
        let decoded: RedactedDiagnostic = serde_json::from_str(&serialized).unwrap();
        assert_eq!(decoded, redacted);
    }

    #[test]
    fn input_cutoff_does_not_preserve_a_partial_provider_key() {
        let compressible_chunk = format!("api_key={}\n", "q".repeat(512));
        let desired_prefix_length = MAX_DIAGNOSTIC_INPUT_BYTES - 10;
        let mut input = String::new();
        while input.len() + compressible_chunk.len() <= desired_prefix_length {
            input.push_str(&compressible_chunk);
        }
        input.push_str(&"x".repeat(desired_prefix_length - input.len()));
        input.push(' ');
        input.push_str("sk-12345678901234567890");

        let redacted = RedactedDiagnostic::new(&input);

        assert_omits_sensitive_test_data(redacted.as_str(), &["sk-123456"]);
        assert!(redacted.as_str().contains(REDACTED));
        assert!(redacted.as_str().ends_with(TRUNCATED));
    }

    #[test]
    fn leaves_unrelated_diagnostic_text_intact() {
        let input = "ACP frame 12 was malformed near method x.ai/session-update";

        assert_eq!(redact_with_home(input), input);
    }
}
