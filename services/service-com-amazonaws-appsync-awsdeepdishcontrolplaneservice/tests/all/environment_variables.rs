// SPDX-License-Identifier: CC0-1.0

//! The API's environmental variable list, end to end over HTTP:
//! `PutGraphqlApiEnvironmentVariables` and `GetGraphqlApiEnvironmentVariables`.
//!
//! Three rules are asserted here, each over a TABLE that spans the class it
//! generalises over rather than over a single request:
//!
//! 1. a write REPLACES the whole list stored for the API it names, and a later read
//!    reports exactly the pairs the last write left — every key and every value
//!    verbatim, whatever characters they hold — without changing them, so a second
//!    read answers the same list. The rows differ along what the rule generalises
//!    over: the entry count (none, one, the modelled maximum of 50), key length on
//!    both modelled bounds, the key alphabet, value length on both modelled bounds
//!    (the empty value included) and the value's character classes, non-ASCII scripts,
//!    emoji and control characters among them;
//! 2. a key outside `EnvironmentVariableKey`'s bounds or alphabet is a 400
//!    `BadRequestException` naming the rejected key, and the refused call leaves the
//!    stored list exactly as it was. The rows sit one past each published length bound
//!    and one per character class the alphabet excludes;
//! 3. the list belongs to the API it was written on: each of several APIs in one store
//!    reports its own list, an API never written to reports an empty one, and an
//!    `apiId` that names no API is refused the way every `apiId`-resolving operation
//!    refuses it (404, or 400 for an id longer than an AppSync API id).
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the boundary validator, the handlers
//! and the shared engine together. The suite reads the error-type header, which the
//! lifecycle suite's helpers do not carry, so it keeps its own `call` for the
//! environmental-variable routes and reuses that suite's router and create helper.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::graphql_api_lifecycle::{create_api, router, REGION};

/// The modelled maximum number of entries an `EnvironmentVariableMap` holds. Source:
/// `smithy_query(query="shape", shape="EnvironmentVariableMap")` — `@length` min 0,
/// max 50, so the empty map is a list too.
const ENTRY_COUNT_MAX: usize = 50;

/// `EnvironmentVariableKey`'s modelled minimum length. Source:
/// `smithy_query(query="shape", shape="EnvironmentVariableKey")` — `@length` min 2,
/// max 64, `@pattern` `^[A-Za-z]+\w*$`.
const KEY_MIN_LEN: usize = 2;

/// `EnvironmentVariableKey`'s modelled maximum length ([`KEY_MIN_LEN`]).
const KEY_MAX_LEN: usize = 64;

/// `EnvironmentVariableValue`'s modelled maximum length. Source:
/// `smithy_query(query="shape", shape="EnvironmentVariableValue")` — `@length` min 0,
/// max 512, measured in characters.
const VALUE_MAX_LEN: usize = 512;

/// An `apiId`-shaped id (26 characters) that no create ever mints here.
const ABSENT_API_ID: &str = "abcdefghijklmnopqrstuvwxyz";

/// One answer from the emulator: the parts a client branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves the error shape from.
    error_type: String,
    /// The error body's `message`, or the empty string when there is none.
    message: String,
    body: Value,
}

/// Send one request to an environmental-variable route and return what the emulator
/// answered.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    api_id: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://appsync.{REGION}.amazonaws.com/v1/apis/{api_id}/environmentVariables"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_type,
        message,
        body,
    }
}

/// `PUT /v1/apis/{apiId}/environmentVariables` with `pairs` as the whole list.
async fn put(router: &MultiServiceRouter, api_id: &str, pairs: &Value) -> Answer {
    call(
        router,
        "PUT",
        api_id,
        Some(json!({ "environmentVariables": pairs })),
    )
    .await
}

/// `GET /v1/apis/{apiId}/environmentVariables`.
async fn get(router: &MultiServiceRouter, api_id: &str) -> Answer {
    call(router, "GET", api_id, None).await
}

/// Create one GraphQL API and return the `apiId` the service minted.
async fn api(router: &MultiServiceRouter, name: &str) -> String {
    let created = create_api(
        router,
        json!({ "name": name, "authenticationType": "API_KEY" }),
    )
    .await;
    created["apiId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: the created API carries an apiId: {created:?}"))
        .to_string()
}

/// `pairs` as the JSON object the wire carries.
fn object(pairs: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// One row of the round-trip table: a label and the pairs it writes.
#[derive(Debug)]
struct Case {
    name: String,
    pairs: Vec<(String, String)>,
}

impl Case {
    fn new(name: &str, pairs: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_string(),
            pairs: pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    fn expected(&self) -> Value {
        object(&self.pairs)
    }
}

/// The table: every class the round-trip rule generalises over.
fn round_trip_cases() -> Vec<Case> {
    let min_key = "k".repeat(KEY_MIN_LEN);
    let max_key = "k".repeat(KEY_MAX_LEN);
    let max_value = "v".repeat(VALUE_MAX_LEN);
    // A value at the modelled maximum made of multi-byte characters: the bound is a
    // CHARACTER count, so this is as long as the ASCII row above and three times its
    // size in bytes.
    let max_multibyte_value = "日".repeat(VALUE_MAX_LEN);
    let mut cases = vec![
        // The modelled minimum entry count IS the empty map.
        Case::new("no pairs at all", &[]),
        Case::new("one pair", &[("ENV", "prod")]),
        Case::new("two pairs", &[("ENV", "prod"), ("Region_1", "eu-west-1")]),
        // `EnvironmentVariableKey`'s modelled length bounds.
        Case::new(
            "key at the minimum length of 2",
            &[(min_key.as_str(), "short key")],
        ),
        Case::new(
            "key at the maximum length of 64",
            &[(max_key.as_str(), "long key")],
        ),
        // The alphabet the key pattern admits: a leading letter of either case, then
        // letters, digits and underscores.
        Case::new(
            "keys spanning the admitted alphabet",
            &[
                ("lowercase_only", "a"),
                ("UPPERCASE_ONLY", "b"),
                ("MixedCase_9_0", "c"),
                ("x_", "d"),
                ("Ab", "e"),
            ],
        ),
        // `EnvironmentVariableValue`'s modelled length bounds — the minimum IS the
        // empty string.
        Case::new("value at the minimum length of 0", &[("EMPTY_VALUE", "")]),
        Case::new(
            "value at the maximum length of 512",
            &[("LONG_VALUE", max_value.as_str())],
        ),
        Case::new(
            "value of 512 multi-byte characters",
            &[("LONG_UNICODE_VALUE", max_multibyte_value.as_str())],
        ),
        // A value is "considered a string literal and not expanded", so whatever
        // characters it holds come back unchanged: scripts outside ASCII, combining
        // marks, emoji, and the characters a JSON body has to escape.
        Case::new(
            "values outside ASCII",
            &[
                ("JAPANESE", "日本語のテキスト"),
                ("CYRILLIC", "русский текст"),
                ("GREEK_MATH", "Ω≈ç√∫˜µ≤≥÷"),
                ("ACCENTED", "café naïve — jamón"),
                ("COMBINING", "e\u{301}galite\u{301}"),
                ("EMOJI", "🎉🚀 ship it 🧑‍🚀"),
                ("RIGHT_TO_LEFT", "مرحبا بالعالم"),
                ("ZERO_WIDTH", "a\u{200b}b"),
            ],
        ),
        Case::new(
            "values holding characters a JSON body escapes",
            &[
                ("QUOTED", "a \"quoted\" value"),
                ("BACKSLASH", r"C:\path\to\thing"),
                ("NEWLINE", "first\nsecond"),
                ("TAB", "left\tright"),
                ("BRACES", "{\"key\":\"value\"}"),
            ],
        ),
        // A value that looks like a reference is a literal too: "environmental
        // variables only support string values ... not expanded".
        Case::new(
            "values that look like references",
            &[("TEMPLATE", "${ctx.args.id}"), ("DOLLAR", "$HOME")],
        ),
    ];
    // The modelled maximum entry count.
    cases.push(Case {
        name: format!("the maximum of {ENTRY_COUNT_MAX} pairs"),
        pairs: (0..ENTRY_COUNT_MAX)
            .map(|index| (format!("Key_{index:02}"), format!("value-{index:02}")))
            .collect(),
    });
    cases
}

/// Rule 1: what a write stores is what a read reports, verbatim and unchanged.
#[tokio::test]
async fn a_written_list_is_the_list_a_later_read_reports() {
    for case in round_trip_cases() {
        let name = case.name.clone();
        let router = router();
        let api_id = api(&router, "Api_env_round_trip").await;
        let expected = case.expected();

        let written = put(&router, &api_id, &expected).await;
        assert_eq!(
            written.status, 200,
            "case {name}: expected PutGraphqlApiEnvironmentVariables 200, got {written:?}"
        );
        assert_eq!(
            written.body["environmentVariables"], expected,
            "case {name}: expected the write to answer the stored list, got {written:?}"
        );

        let read = get(&router, &api_id).await;
        assert_eq!(
            read.status, 200,
            "case {name}: expected GetGraphqlApiEnvironmentVariables 200, got {read:?}"
        );
        assert_eq!(
            read.body["environmentVariables"], expected,
            "case {name}: expected the stored list, got {read:?}"
        );

        // A read may not change what a later caller sees.
        let again = get(&router, &api_id).await;
        assert_eq!(
            again.body["environmentVariables"], expected,
            "case {name}: expected a second read to report the same list, got {again:?}"
        );
    }
}

/// Rule 1, continued: a write OVERWRITES the stored list rather than merging into it,
/// so a key the request omits is gone. The steps differ along what changes between two
/// writes: a value, a key set, a shrink to nothing, and a regrow from nothing.
#[tokio::test]
async fn a_write_replaces_the_whole_stored_list() {
    let router = router();
    let api_id = api(&router, "Api_env_overwrite").await;

    let steps = [
        (
            "the first list",
            json!({ "ENV": "prod", "TEAM": "search" }),
        ),
        (
            "one value changed, same keys",
            json!({ "ENV": "staging", "TEAM": "search" }),
        ),
        (
            "a disjoint key set",
            json!({ "REGION_1": "eu-west-1" }),
        ),
        (
            "a subset of the previous keys plus a new one",
            json!({ "REGION_1": "eu-west-1", "TIER": "gold" }),
        ),
        ("emptied", json!({})),
        ("regrown from empty", json!({ "ENV": "dev" })),
    ];

    for (name, list) in &steps {
        let written = put(&router, &api_id, list).await;
        assert_eq!(
            written.status, 200,
            "case {name}: expected PutGraphqlApiEnvironmentVariables 200, got {written:?}"
        );
        assert_eq!(
            &written.body["environmentVariables"], list,
            "case {name}: expected the write to answer the submitted list, got {written:?}"
        );
        let read = get(&router, &api_id).await;
        assert_eq!(
            &read.body["environmentVariables"], list,
            "case {name}: expected the stored list to be exactly the last one written, \
             got {read:?}"
        );
    }
}

/// Rule 2: a key outside `EnvironmentVariableKey`'s bounds or alphabet is refused, and
/// the refused call stores nothing.
///
/// The rows sit one past each published length bound and one per character class the
/// alphabet excludes; the accepted side of both bounds is covered by the round-trip
/// table above.
#[tokio::test]
async fn a_key_outside_the_modelled_key_rules_is_refused_and_stores_nothing() {
    let router = router();
    let api_id = api(&router, "Api_env_bad_keys").await;

    // The list the API holds before each refused write, so the refusal can be shown to
    // have changed nothing.
    let stored = json!({ "ENV": "prod", "Region_1": "eu-west-1" });
    let seeded = put(&router, &api_id, &stored).await;
    assert_eq!(
        seeded.status, 200,
        "case seed the stored list: expected 200, got {seeded:?}"
    );

    let too_short = "k".repeat(KEY_MIN_LEN - 1);
    let too_long = "k".repeat(KEY_MAX_LEN + 1);
    let rows: Vec<(&str, String)> = vec![
        ("one short of the minimum length", too_short),
        ("one past the maximum length", too_long),
        ("beginning with a digit", "1KEY".to_string()),
        ("beginning with an underscore", "_KEY".to_string()),
        ("holding a hyphen", "MY-KEY".to_string()),
        ("holding a dot", "MY.KEY".to_string()),
        ("holding a space", "MY KEY".to_string()),
        ("holding a slash", "MY/KEY".to_string()),
        ("holding a dollar sign", "MY$KEY".to_string()),
        ("beginning with a non-ASCII letter", "Ébauche".to_string()),
        ("outside ASCII entirely", "日本語".to_string()),
    ];

    for (label, key) in &rows {
        // The offending key rides beside a well-formed one, so the refusal is about
        // the key rather than about the request holding one entry.
        let list = object(&[
            ("GOOD_KEY".to_string(), "fine".to_string()),
            (key.clone(), "value".to_string()),
        ]);
        let answer = put(&router, &api_id, &list).await;
        assert_eq!(
            answer.status, 400,
            "case a key {label} ({key:?}): expected status 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case a key {label} ({key:?}): expected error type BadRequestException, \
             got {answer:?}"
        );
        assert!(
            answer.message.contains(key.as_str()),
            "case a key {label} ({key:?}): expected the refusal to name the rejected key, \
             got {answer:?}"
        );

        let read = get(&router, &api_id).await;
        assert_eq!(
            read.body["environmentVariables"], stored,
            "case a key {label} ({key:?}): expected the refused write to store nothing, \
             got {read:?}"
        );
    }
}

/// Rule 3: the list belongs to the API it was written on, and an `apiId` that names no
/// API is refused the way every `apiId`-resolving operation refuses it.
#[tokio::test]
async fn each_api_reports_its_own_list() {
    let router = router();
    let rows = [
        ("first", json!({ "ENV": "prod", "TEAM": "search" })),
        ("second", json!({ "ENV": "staging" })),
        ("third", json!({ "OWNER": "platform", "Cost_Center": "42" })),
    ];

    let mut ids = Vec::new();
    for (name, list) in &rows {
        let api_id = api(&router, &format!("Api_env_{name}")).await;
        let written = put(&router, &api_id, list).await;
        assert_eq!(
            written.status, 200,
            "case {name}: expected PutGraphqlApiEnvironmentVariables 200, got {written:?}"
        );
        ids.push(api_id);
    }
    // An API nobody wrote to: it holds the empty list, not another API's.
    let untouched = api(&router, "Api_env_untouched").await;

    for ((name, list), api_id) in rows.iter().zip(&ids) {
        let read = get(&router, api_id).await;
        assert_eq!(
            read.status, 200,
            "case {name}: expected GetGraphqlApiEnvironmentVariables 200, got {read:?}"
        );
        assert_eq!(
            &read.body["environmentVariables"], list,
            "case {name}: expected only this API's list, got {read:?}"
        );
    }

    let read = get(&router, &untouched).await;
    assert_eq!(
        read.status, 200,
        "case an API never written to: expected 200, got {read:?}"
    );
    assert_eq!(
        read.body["environmentVariables"],
        json!({}),
        "case an API never written to: expected the empty list, got {read:?}"
    );

    // An id that names no API: not-found for an id an API could have, and a bad
    // request for one longer than an AppSync API id ever is, on BOTH operations.
    let deleted = api(&router, "Api_env_deleted").await;
    let dropped = crate::graphql_api_lifecycle::call(
        &router,
        "DELETE",
        &format!("/v1/apis/{deleted}"),
        None,
    )
    .await;
    assert_eq!(
        dropped.0, 200,
        "case delete an API: expected 200, got {dropped:?}"
    );

    let absent = [
        ("an id no create ever minted", ABSENT_API_ID.to_string()),
        ("a deleted API's id", deleted),
        ("a single character", "a".to_string()),
    ];
    for (label, api_id) in &absent {
        for (operation, answer) in [
            ("GetGraphqlApiEnvironmentVariables", get(&router, api_id).await),
            (
                "PutGraphqlApiEnvironmentVariables",
                put(&router, api_id, &json!({ "ENV": "prod" })).await,
            ),
        ] {
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (404, "NotFoundException"),
                "case {operation} with {label} ({api_id:?}): expected 404 NotFoundException, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.message, "API not found.",
                "case {operation} with {label} ({api_id:?}): expected AppSync's fixed \
                 not-found text, got {answer:?}"
            );
        }
    }

    // One character past the 26 an AppSync API id has: the id names no API at all, so
    // the call is malformed rather than a miss.
    let overlong = "d".repeat(27);
    for (operation, answer) in [
        (
            "GetGraphqlApiEnvironmentVariables",
            get(&router, &overlong).await,
        ),
        (
            "PutGraphqlApiEnvironmentVariables",
            put(&router, &overlong, &json!({ "ENV": "prod" })).await,
        ),
    ] {
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "BadRequestException"),
            "case {operation} with an overlong apiId: expected 400 BadRequestException, \
             got {answer:?}"
        );
        assert_eq!(
            answer.message,
            format!("Invalid API ID {overlong}"),
            "case {operation} with an overlong apiId: expected the refusal to name the \
             rejected id, got {answer:?}"
        );
    }
}

/// The modelled entry count is enforced too: one pair past the maximum is refused by
/// the request boundary (which owns every modelled `@length`), so nothing is stored.
#[tokio::test]
async fn one_pair_past_the_modelled_maximum_is_refused() {
    let router = router();
    let api_id = api(&router, "Api_env_too_many").await;

    let pairs: Vec<(String, String)> = (0..=ENTRY_COUNT_MAX)
        .map(|index| (format!("Key_{index:02}"), format!("value-{index:02}")))
        .collect();
    let answer = put(&router, &api_id, &object(&pairs)).await;
    assert_eq!(
        answer.status, 400,
        "case {} pairs: expected status 400, got {answer:?}",
        pairs.len()
    );

    let read = get(&router, &api_id).await;
    assert_eq!(
        read.body["environmentVariables"],
        json!({}),
        "case {} pairs: expected the refused write to store nothing, got {read:?}",
        pairs.len()
    );
}
