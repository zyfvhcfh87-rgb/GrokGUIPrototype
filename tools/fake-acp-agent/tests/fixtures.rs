use std::collections::BTreeSet;

use serde_json::Value;

#[test]
fn checked_in_transcripts_are_valid_sanitized_jsonl() {
    let lifecycle = include_str!("../../../fixtures/acp/lifecycle.jsonl");
    let mut methods = BTreeSet::new();
    let mut lifecycle_records = Vec::new();

    for (index, line) in lifecycle.lines().enumerate() {
        let record: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("lifecycle line {} is invalid: {error}", index + 1));
        if let Some(method) = record.pointer("/frame/method").and_then(Value::as_str) {
            methods.insert(method.to_owned());
        }
        lifecycle_records.push(record);
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

    assert_exchange_has_session_update(&lifecycle_records, "session/load", true);
    assert_exchange_has_session_update(&lifecycle_records, "session/resume", false);

    for (name, fixture) in [
        ("lifecycle", lifecycle),
        (
            "malformed",
            include_str!("../../../fixtures/acp/malformed.jsonl"),
        ),
        ("stderr", include_str!("../../../fixtures/acp/stderr.jsonl")),
        ("crash", include_str!("../../../fixtures/acp/crash.jsonl")),
    ] {
        for (index, line) in fixture.lines().enumerate() {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("{name} line {} is invalid: {error}", index + 1));
        }
        assert_fixture_is_private(name, fixture);
    }

    for (name, fixture) in [
        (
            "installed initialize",
            include_str!("../../../fixtures/acp/installed-grok-1.0.5-initialize-shape.json"),
        ),
        (
            "installed controls",
            include_str!("../../../fixtures/acp/installed-grok-1.0.5-controls-shape.json"),
        ),
        (
            "installed lifecycle",
            include_str!("../../../fixtures/acp/installed-grok-1.0.5-lifecycle-shape.json"),
        ),
    ] {
        serde_json::from_str::<Value>(fixture)
            .unwrap_or_else(|error| panic!("{name} fixture is invalid: {error}"));
        assert_fixture_is_private(name, fixture);
    }
}

fn assert_exchange_has_session_update(records: &[Value], method: &str, expected: bool) {
    let request_index = records
        .iter()
        .position(|record| record.pointer("/frame/method").and_then(Value::as_str) == Some(method))
        .unwrap_or_else(|| panic!("fixture omits {method} request"));
    let request_id = records[request_index]
        .pointer("/frame/id")
        .expect("fixture request must have an id");
    let response_index = records
        .iter()
        .enumerate()
        .skip(request_index + 1)
        .find_map(|(index, record)| {
            let frame = record.pointer("/frame")?;
            let is_response = frame.get("result").is_some() || frame.get("error").is_some();
            (is_response && frame.get("id") == Some(request_id)).then_some(index)
        })
        .unwrap_or_else(|| panic!("fixture omits {method} response"));
    let has_update = records[request_index + 1..response_index]
        .iter()
        .any(|record| {
            record.pointer("/frame/method").and_then(Value::as_str) == Some("session/update")
        });

    assert_eq!(
        has_update, expected,
        "{method} replay behavior disagrees with the ACP contract"
    );
}

fn assert_fixture_is_private(name: &str, fixture: &str) {
    let lowercase = fixture.to_ascii_lowercase();
    for forbidden in [
        "aryel",
        r"c:\\users\\",
        "c:/users/",
        r"e:\\grokbuildgui",
        "auth.json",
        "http://",
        "https://",
        "bearer ",
        "xai-",
        "sk-",
    ] {
        assert!(
            !lowercase.contains(forbidden),
            "{name} fixture contains forbidden private material"
        );
    }
    assert!(
        !fixture.as_bytes().windows(3).any(|window| window == b"eyJ"),
        "{name} fixture contains a JWT-like value"
    );
    assert!(
        !contains_uuid(fixture),
        "{name} fixture contains a UUID-like value"
    );
}

fn contains_uuid(value: &str) -> bool {
    value.as_bytes().windows(36).any(|candidate| {
        candidate.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    })
}
