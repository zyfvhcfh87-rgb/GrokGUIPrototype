use std::collections::BTreeSet;

use serde_json::Value;

#[test]
fn checked_in_transcripts_are_valid_sanitized_jsonl() {
    let lifecycle = include_str!("../../../fixtures/acp/lifecycle.jsonl");
    let mut methods = BTreeSet::new();

    for (index, line) in lifecycle.lines().enumerate() {
        let record: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("lifecycle line {} is invalid: {error}", index + 1));
        if let Some(method) = record.pointer("/frame/method").and_then(Value::as_str) {
            methods.insert(method.to_owned());
        }
    }

    for required in [
        "initialize",
        "authenticate",
        "session/new",
        "session/prompt",
        "session/update",
        "session/request_permission",
        "elicitation/create",
        "session/cancel",
        "session/list",
        "session/load",
        "session/resume",
        "session/close",
    ] {
        assert!(methods.contains(required), "fixture omits {required}");
    }

    for fixture in [
        lifecycle,
        include_str!("../../../fixtures/acp/malformed.jsonl"),
        include_str!("../../../fixtures/acp/stderr.jsonl"),
        include_str!("../../../fixtures/acp/crash.jsonl"),
    ] {
        for (index, line) in fixture.lines().enumerate() {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("fixture line {} is invalid: {error}", index + 1));
        }
        assert!(!fixture.contains("Aryel"));
        assert!(!fixture.contains("Users\\"));
        assert!(!fixture.contains("auth.json"));
    }
}
