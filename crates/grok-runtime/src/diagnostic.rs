use std::{fmt, path::Path};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::executable::current_home_dir;

const REDACTED: &str = "[REDACTED]";

const SECRET_LABELS: &[SecretLabel] = &[
    SecretLabel::new("proxy-authorization", true),
    SecretLabel::new("refresh_token", false),
    SecretLabel::new("refresh-token", false),
    SecretLabel::new("access_token", false),
    SecretLabel::new("access-token", false),
    SecretLabel::new("client_secret", false),
    SecretLabel::new("client-secret", false),
    SecretLabel::new("authorization", true),
    SecretLabel::new("x-api-key", false),
    SecretLabel::new("auth_token", false),
    SecretLabel::new("auth-token", false),
    SecretLabel::new("api_key", false),
    SecretLabel::new("api-key", false),
    SecretLabel::new("api key", false),
    SecretLabel::new("apikey", false),
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

    #[cfg(test)]
    fn with_home(raw: &str, home: Option<&Path>) -> Self {
        Self(redact_with_home(raw, home))
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

/// Redact common credential forms and the current user's home path from text
/// before it is displayed or persisted as diagnostic data.
pub fn redact_diagnostic(input: &str) -> String {
    let home = current_home_dir();
    redact_with_home(input, home.as_deref())
}

fn redact_with_home(input: &str, home: Option<&Path>) -> String {
    let mut redacted = redact_home_path(input, home);

    for label in SECRET_LABELS {
        redacted = redact_secret_label(&redacted, *label);
    }

    redacted = redact_bearer_tokens(&redacted);
    redact_long_key_prefixes(&redacted)
}

fn redact_home_path(input: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return input.to_owned();
    };

    let home = home.as_os_str().to_string_lossy();
    if home.is_empty() || home == "/" || is_windows_drive_root(&home) {
        return input.to_owned();
    }

    let mut variants = vec![
        home.to_string(),
        home.replace('\\', "/"),
        home.replace('/', "\\"),
    ];
    variants.sort_by_key(|variant| std::cmp::Reverse(variant.len()));
    variants.dedup();

    variants
        .into_iter()
        .fold(input.to_owned(), |text, variant| {
            replace_home_variant(&text, &variant)
        })
}

fn is_windows_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn replace_home_variant(input: &str, home: &str) -> String {
    let case_insensitive = cfg!(windows) || home.contains('\\');
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut search_from = 0;

    while let Some(start) = find_text(input, home, search_from, case_insensitive) {
        let end = start + home.len();

        if home_match_has_boundaries(input, start, end) {
            output.push_str(&input[copied_through..start]);
            output.push('~');
            copied_through = end;
            search_from = end;
        } else {
            search_from = next_char_boundary(input, start);
        }
    }

    output.push_str(&input[copied_through..]);
    output
}

fn home_match_has_boundaries(input: &str, start: usize, end: usize) -> bool {
    let before_is_safe = input[..start]
        .chars()
        .next_back()
        .is_none_or(|character| !is_identifier_character(character));
    let after_is_safe = input[end..]
        .chars()
        .next()
        .is_none_or(|character| !is_identifier_character(character));

    before_is_safe && after_is_safe
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

    if !matches!(byte_at(input, cursor), Some(b':' | b'=')) {
        return None;
    }

    cursor += 1;
    cursor = skip_ascii_whitespace(input, cursor);
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
        None => unquoted_value_end(input, value_start),
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

fn redact_long_key_prefixes(input: &str) -> String {
    ["xai-", "sk-"]
        .into_iter()
        .fold(input.to_owned(), |text, prefix| {
            redact_long_key_prefix(&text, prefix)
        })
}

fn redact_long_key_prefix(input: &str, prefix: &str) -> String {
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

        if end - start < prefix.len() + 16 {
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

    #[test]
    fn redacts_bearer_and_common_api_key_forms() {
        let input = concat!(
            "Authorization: Bearer header-secret\n",
            "x-api-key = top-secret\n",
            "{\"apiKey\":\"json-secret\"}\n",
            "XAI_API_KEY=environment-secret\n",
            "standalone bearer loose-secret"
        );

        let redacted = redact_with_home(input, None);

        assert!(!redacted.contains("header-secret"));
        assert!(!redacted.contains("top-secret"));
        assert!(!redacted.contains("json-secret"));
        assert!(!redacted.contains("environment-secret"));
        assert!(!redacted.contains("loose-secret"));
        assert_eq!(redacted.matches(REDACTED).count(), 5);
    }

    #[test]
    fn redacts_long_provider_key_prefixes_but_not_short_technical_names() {
        let input = "key xai-1234567890abcdefghijkl and package sk-learn";

        let redacted = redact_with_home(input, None);

        assert_eq!(redacted, "key [REDACTED] and package sk-learn");
    }

    #[test]
    fn redacts_home_with_native_and_alternate_separators() {
        let home = Path::new(r"C:\Users\Ada");
        let input = r"failed at C:\Users\Ada\.grok and C:/Users/Ada/project";

        let redacted = RedactedDiagnostic::with_home(input, Some(home));

        assert_eq!(redacted.as_str(), r"failed at ~\.grok and ~/project");
    }

    #[test]
    fn does_not_redact_a_longer_username_with_the_same_prefix() {
        let home = Path::new("/home/ada");

        let redacted =
            RedactedDiagnostic::with_home("owners: /home/ada and /home/adam/project", Some(home));

        assert_eq!(redacted.as_str(), "owners: ~ and /home/adam/project");
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
    fn leaves_unrelated_diagnostic_text_intact() {
        let input = "ACP frame 12 was malformed near method x.ai/session-update";

        assert_eq!(redact_with_home(input, None), input);
    }
}
