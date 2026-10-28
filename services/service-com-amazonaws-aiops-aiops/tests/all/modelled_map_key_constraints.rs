// SPDX-License-Identifier: CC0-1.0

//! The modelled constraints a MAP's KEY shape declares, applied to every entry key a
//! request carries.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! deserialization (including the `@httpLabel` `resourceArn`/`identifier`), the
//! centralized constraint walk and this service's handlers — so what is asserted is what
//! a signed AWS SDK request observes.
//!
//! The rule under test: a map's KEY shape binds every entry key of that map exactly as a
//! value shape binds its values, so a key outside the key shape's modelled `@length` or
//! `@pattern` is refused with a 400 `ValidationException` naming the map member, and
//! nothing of the request is stored. The centralized walk never reaches a map key, so the
//! rule is applied by the handlers that accept a map; `tests/all/map_member_refusal_paths.rs`
//! pins how a map VALUE's refusal is worded, and this suite pins the key side of it.
//!
//! The rows differ along what the rule generalises over: the MEMBER and its key shape
//! (`tags`, keyed by `TagKey`; `chatbotNotificationChannel`, keyed by `SNSTopicArn`), the
//! OPERATIONS that accept either (`CreateInvestigationGroup`, `UpdateInvestigationGroup`,
//! `TagResource`), the CONSTRAINT broken (each published length bound, the character
//! class, and two at once), how FAR past a bound the key is (one character and far
//! beyond), the number of broken members in one request, and — on the accepting side —
//! keys ON each published bound and keys whose characters span the modelled class.
//!
//! An account holds ONE investigation group per Region, so a row that needs its own group
//! signs for its own Region.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the group every row reads and writes lives in.
const REGION: &str = "us-west-2";

/// The Region the `CreateInvestigationGroup` rows sign for: their own, so a refusal can
/// only have come from the rule under test and never from the per-Region group quota.
const CREATE_REGION: &str = "eu-central-1";

/// A role ARN that satisfies the modelled `RoleArn` (`@length` 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// A Chatbot configuration ARN a `chatbotNotificationChannel` entry's list may hold,
/// satisfying the modelled `ChatConfigurationArn` (`@length` 20..2048, `^arn:.*$`).
const CHAT_CONFIGURATION_ARN: &str =
    "arn:aws:chatbot::000000000000:chat-configuration/slack-channel/ops";

/// The shortest and longest key the modelled `TagKey` admits
/// (`smithy_query(query="shape",shape="Tags")`: the map's key targets `TagKey`, `@length`
/// 1..128).
const TAG_KEY_MIN_LENGTH: usize = 1;
const TAG_KEY_MAX_LENGTH: usize = 128;

/// The shortest and longest key the modelled `SNSTopicArn` admits
/// (`smithy_query(query="shape",shape="ChatbotNotificationChannel")`: the map's key
/// targets `SNSTopicArn`, `@length` 20..2048).
const TOPIC_ARN_MIN_LENGTH: usize = 20;
const TOPIC_ARN_MAX_LENGTH: usize = 2048;

/// The requirements a tag KEY states, as a clause bundles them at the map member: both
/// published bounds and the character class, from `TagKey`'s own constraints (`@pattern`
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`, reported without the anchors a model spells a
/// whole-string match with).
const TAG_KEY_REQUIREMENTS: &[&str] = &[
    "Member must have length less than or equal to 128",
    "Member must have length greater than or equal to 1",
    r"Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]+)",
];

/// The requirements an SNS topic KEY states, from `SNSTopicArn`'s own constraints
/// (`@length` 20..2048, `@pattern` `^arn:.*$`).
const TOPIC_KEY_REQUIREMENTS: &[&str] = &[
    "Member must have length less than or equal to 2048",
    "Member must have length greater than or equal to 20",
    "Member must satisfy regular expression pattern: arn:.*",
];

/// The requirements a tag VALUE states, from `TagValue` (`@length` 1..256, `@pattern`
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) — the control rows' expectation, unchanged by the
/// rule under test.
const TAG_VALUE_REQUIREMENTS: &[&str] = &[
    "Member must have length less than or equal to 256",
    "Member must have length greater than or equal to 1",
    r"Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*)",
];

/// The error code a modelled-constraint refusal carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code the tagging frontend refuses a tag write with for a rule the model
/// does not express (the reserved key namespace); `tests/all/resource_tags.rs` owns its
/// wording.
const TAGGING_BAD_REQUEST: &str = "BadRequestException";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status, error code, `message` and parsed body of one call.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, String, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let header_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(header_type)
        .unwrap_or_default()
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message, parsed)
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The whole message a map member's KEY refusal states: the member, and the bundle of
/// requirements its key shape states.
fn key_message(member_path: &str, requirements: &[&str]) -> String {
    format!(
        "1 validation error detected: {}",
        key_clause(member_path, requirements)
    )
}

/// One clause of a KEY refusal.
fn key_clause(member_path: &str, requirements: &[&str]) -> String {
    format!(
        "Value at '{member_path}' failed to satisfy constraint: Map keys must satisfy \
         constraint: [{}]",
        requirements.join(", ")
    )
}

/// The whole message a map member's VALUE refusal states — the control rows'
/// expectation.
fn value_message(member_path: &str, requirements: &[&str]) -> String {
    format!(
        "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
         Map value must satisfy constraint: [{}]",
        requirements.join(", ")
    )
}

/// A `CreateInvestigationGroup` body carrying `members` beside the required ones.
fn create_body(members: Vec<(&str, Value)>) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!("map-key-rules"));
    body.insert("roleArn".to_string(), json!(ROLE_ARN));
    for (member, value) in members {
        body.insert(member.to_string(), value);
    }
    Value::Object(body)
}

/// Create the one group of `region` and answer its ARN.
async fn create_group(
    router: &MultiServiceRouter,
    region: &str,
    members: Vec<(&str, Value)>,
) -> String {
    let (status, code, message, body) = call(
        router,
        region,
        "POST",
        "/investigationGroups",
        Some(&create_body(members)),
    )
    .await;
    assert_eq!(
        status, 201,
        "creating the group in {region}: expected 201, got {status} / {code} / {message:?}"
    );
    body.get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("creating the group in {region}: no arn in {body:?}"))
        .to_string()
}

/// A tag key no modelled `TagKey` admits, as `(case, key)`: each published bound missed
/// by one character and by far more, the character class broken by several different
/// characters, and both broken at once.
fn refused_tag_keys() -> Vec<(&'static str, String)> {
    vec![
        (
            "a key one character past the maximum",
            "k".repeat(TAG_KEY_MAX_LENGTH + 1),
        ),
        ("a key far past the maximum", "k".repeat(500)),
        ("a key below the minimum", String::new()),
        ("a key outside the character class", "bad key!".to_string()),
        ("a key carrying a star", "cost*centre".to_string()),
        ("a key carrying a tab", "cost\tcentre".to_string()),
        ("a key carrying a comma", "cost,centre".to_string()),
        (
            "a key breaking the maximum and the class at once",
            "!".repeat(300),
        ),
    ]
}

/// An SNS topic key no modelled `SNSTopicArn` admits, as `(case, key)`: the same classes
/// one shape further out — each bound missed by one character and by far more, and the
/// `^arn:` prefix the pattern demands.
fn refused_topic_keys() -> Vec<(&'static str, String)> {
    vec![
        (
            "a topic key one character below the minimum",
            format!("arn:{}", "a".repeat(TOPIC_ARN_MIN_LENGTH - 5)),
        ),
        ("a topic key far below the minimum", "arn:a".to_string()),
        (
            "a topic key one character past the maximum",
            format!("arn:{}", "a".repeat(TOPIC_ARN_MAX_LENGTH - 3)),
        ),
        (
            "a topic key that is not an ARN",
            "not-an-arn-but-long-enough".to_string(),
        ),
        ("a topic key below the minimum AND not an ARN", "nope".to_string()),
    ]
}

/// A tag key every modelled `TagKey` admits, as `(case, key)`: each published bound ON
/// the bound, and keys spanning the modelled character class (letters of several scripts,
/// digits, the separator, and each punctuation character the class names).
fn accepted_tag_keys() -> Vec<(&'static str, String)> {
    vec![
        ("a key at the minimum", "k".repeat(TAG_KEY_MIN_LENGTH)),
        ("a key at the maximum", "k".repeat(TAG_KEY_MAX_LENGTH)),
        ("an ordinary key", "CostCentre-4711".to_string()),
        ("a key carrying the path separator", "cost.centre".to_string()),
        ("a key carrying a space", "cost centre".to_string()),
        (
            "a key carrying every punctuation character the class names",
            "a_b.c:d/e=f+g-h@i".to_string(),
        ),
        ("a non-ASCII key", "Ωmega.Проект".to_string()),
        ("a key that starts with aws but not the reserved prefix", "awsome".to_string()),
    ]
}

/// An SNS topic key every modelled `SNSTopicArn` admits, as `(case, key)`: both published
/// bounds ON the bound, and an ordinary topic ARN.
fn accepted_topic_keys() -> Vec<(&'static str, String)> {
    vec![
        (
            "a topic key at the minimum",
            format!("arn:{}", "a".repeat(TOPIC_ARN_MIN_LENGTH - 4)),
        ),
        (
            "a topic key at the maximum",
            format!("arn:{}", "a".repeat(TOPIC_ARN_MAX_LENGTH - 4)),
        ),
        (
            "an ordinary topic ARN",
            "arn:aws:sns:us-west-2:000000000000:my-private-topic".to_string(),
        ),
    ]
}

/// One row: the case, the request, the error code and the message the service must answer
/// with (`None` where another suite owns the wording).
struct Case {
    name: String,
    region: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    code: &'static str,
    expected: Option<String>,
}

#[tokio::test]
async fn a_map_key_outside_its_modelled_key_shape_is_refused_and_nothing_is_stored() {
    let router = emulator();
    let arn = create_group(&router, REGION, Vec::new()).await;
    let tags_uri = format!("/tags/{}", label(&arn));
    let group_uri = format!("/investigationGroups/{}", label(&arn));
    let mut cases: Vec<Case> = Vec::new();

    // A `tags` key, on both operations that accept a `tags` map.
    for (case, key) in refused_tag_keys() {
        cases.push(Case {
            name: format!("TagResource with {case}"),
            region: REGION,
            method: "POST",
            uri: tags_uri.clone(),
            body: Some(json!({ "tags": { key.clone(): "v" } })),
            code: VALIDATION_EXCEPTION,
            expected: Some(key_message("tags", TAG_KEY_REQUIREMENTS)),
        });
        cases.push(Case {
            name: format!("CreateInvestigationGroup with {case}"),
            region: CREATE_REGION,
            method: "POST",
            uri: "/investigationGroups".to_string(),
            body: Some(create_body(vec![("tags", json!({ key.clone(): "v" }))])),
            code: VALIDATION_EXCEPTION,
            expected: Some(key_message("tags", TAG_KEY_REQUIREMENTS)),
        });
    }

    // A `chatbotNotificationChannel` key, on both operations that accept that map.
    for (case, key) in refused_topic_keys() {
        let channel = json!({ key.clone(): [CHAT_CONFIGURATION_ARN] });
        cases.push(Case {
            name: format!("CreateInvestigationGroup with {case}"),
            region: CREATE_REGION,
            method: "POST",
            uri: "/investigationGroups".to_string(),
            body: Some(create_body(vec![(
                "chatbotNotificationChannel",
                channel.clone(),
            )])),
            code: VALIDATION_EXCEPTION,
            expected: Some(key_message(
                "chatbotNotificationChannel",
                TOPIC_KEY_REQUIREMENTS,
            )),
        });
        cases.push(Case {
            name: format!("UpdateInvestigationGroup with {case}"),
            region: REGION,
            method: "PATCH",
            uri: group_uri.clone(),
            body: Some(json!({ "chatbotNotificationChannel": channel })),
            code: VALIDATION_EXCEPTION,
            expected: Some(key_message(
                "chatbotNotificationChannel",
                TOPIC_KEY_REQUIREMENTS,
            )),
        });
    }

    // Two members broken in one request: one clause each, counted by the envelope and
    // ordered as the input declares the members.
    cases.push(Case {
        name: "CreateInvestigationGroup with both members' keys broken".to_string(),
        region: CREATE_REGION,
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: Some(create_body(vec![
            ("tags", json!({ "bad key!": "v" })),
            (
                "chatbotNotificationChannel",
                json!({ "not-an-arn-but-long-enough": [CHAT_CONFIGURATION_ARN] }),
            ),
        ])),
        code: VALIDATION_EXCEPTION,
        expected: Some(format!(
            "2 validation errors detected: {}; {}",
            key_clause("tags", TAG_KEY_REQUIREMENTS),
            key_clause("chatbotNotificationChannel", TOPIC_KEY_REQUIREMENTS),
        )),
    });

    // A modelled constraint is what the request boundary applies, so it is applied before
    // a rule the model does not express: a key that is BOTH past the modelled maximum and
    // inside the reserved namespace is the modelled refusal.
    cases.push(Case {
        name: "TagResource with a reserved key past the modelled maximum".to_string(),
        region: REGION,
        method: "POST",
        uri: tags_uri.clone(),
        body: Some(json!({ "tags": { format!("aws:{}", "k".repeat(500)): "v" } })),
        code: VALIDATION_EXCEPTION,
        expected: Some(key_message("tags", TAG_KEY_REQUIREMENTS)),
    });

    // Controls. A key the model admits still reaches the unmodelled tagging rules, and a
    // broken VALUE still reports its own bundle at the same member.
    cases.push(Case {
        name: "TagResource with a reserved key the model admits".to_string(),
        region: REGION,
        method: "POST",
        uri: tags_uri.clone(),
        body: Some(json!({ "tags": { "aws:cloudformation:stack-name": "v" } })),
        code: TAGGING_BAD_REQUEST,
        expected: None,
    });
    cases.push(Case {
        name: "TagResource with a legal key and a broken value".to_string(),
        region: REGION,
        method: "POST",
        uri: tags_uri.clone(),
        body: Some(json!({ "tags": { "CostCentre-4711": "v".repeat(257) } })),
        code: VALIDATION_EXCEPTION,
        expected: Some(value_message("tags", TAG_VALUE_REQUIREMENTS)),
    });

    for case in cases {
        let Case {
            name,
            region,
            method,
            uri,
            body,
            code: expected_code,
            expected,
        } = case;
        let (status, code, message, _) = call(&router, region, method, &uri, body.as_ref()).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} / {code} / {message:?}"
        );
        assert_eq!(
            code, expected_code,
            "case {name:?}: expected {expected_code}, got {code} / {message:?}"
        );
        if let Some(expected) = expected {
            assert_eq!(
                message, expected,
                "case {name:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // Nothing a refused request named was written: the group still carries no tags and no
    // notification channel.
    let (status, code, message, body) = call(&router, REGION, "GET", &tags_uri, None).await;
    assert_eq!(
        status, 200,
        "reading the tags back: expected 200, got {status} / {code} / {message:?}"
    );
    assert_eq!(
        body.get("tags"),
        Some(&json!({})),
        "after the refusals: expected no stored tags, got {body:?}"
    );
    let (status, code, message, body) = call(&router, REGION, "GET", &group_uri, None).await;
    assert_eq!(
        status, 200,
        "reading the group back: expected 200, got {status} / {code} / {message:?}"
    );
    assert_eq!(
        body.get("chatbotNotificationChannel")
            .unwrap_or(&Value::Null),
        &Value::Null,
        "after the refusals: expected no stored notification channel, got {body:?}"
    );
}

#[tokio::test]
async fn a_map_key_inside_its_modelled_key_shape_is_accepted_and_persisted() {
    // The rule refuses what the key shape excludes and NOTHING else, so every key on or
    // inside the published bounds is still stored and read back.
    let router = emulator();
    let arn = create_group(&router, REGION, Vec::new()).await;
    let tags_uri = format!("/tags/{}", label(&arn));
    let group_uri = format!("/investigationGroups/{}", label(&arn));

    for (case, key) in accepted_tag_keys() {
        let value = format!("value for {case}");
        let (status, code, message, _) = call(
            &router,
            REGION,
            "POST",
            &tags_uri,
            Some(&json!({ "tags": { key.clone(): value.clone() } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {code} / {message:?}"
        );
        let (status, code, message, body) = call(&router, REGION, "GET", &tags_uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the tags back expected 200, got {status} / {code} / {message:?}"
        );
        assert_eq!(
            body.get("tags").and_then(|tags| tags.get(&key)),
            Some(&Value::String(value.clone())),
            "case {case:?}: expected the stored value {value:?}, got {body:?}"
        );
    }

    // A create carries the same map, so the keys the tagging path accepts are the keys a
    // create accepts: all of them at once, in their own Region's group.
    let mut tags = Map::new();
    for (case, key) in accepted_tag_keys() {
        tags.insert(key, Value::String(format!("created for {case}")));
    }
    let created = create_group(
        &router,
        CREATE_REGION,
        vec![("tags", Value::Object(tags.clone()))],
    )
    .await;
    let (status, code, message, body) = call(
        &router,
        CREATE_REGION,
        "GET",
        &format!("/tags/{}", label(&created)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "reading a created group's tags: expected 200, got {status} / {code} / {message:?}"
    );
    for (key, value) in tags {
        assert_eq!(
            body.get("tags").and_then(|tags| tags.get(&key)),
            Some(&value),
            "case {key:?}: expected the created value {value:?}, got {body:?}"
        );
    }

    // The other key shape: each accepted topic key is stored as the channel's key and
    // reported back by the reader.
    for (case, key) in accepted_topic_keys() {
        let (status, code, message, _) = call(
            &router,
            REGION,
            "PATCH",
            &group_uri,
            Some(&json!({
                "chatbotNotificationChannel": { key.clone(): [CHAT_CONFIGURATION_ARN] },
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {code} / {message:?}"
        );
        let (status, code, message, body) = call(&router, REGION, "GET", &group_uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the group back expected 200, got {status} / {code} / \
             {message:?}"
        );
        assert_eq!(
            body.get("chatbotNotificationChannel")
                .and_then(|channel| channel.get(&key)),
            Some(&json!([CHAT_CONFIGURATION_ARN])),
            "case {case:?}: expected the stored channel entry, got {body:?}"
        );
    }
}
