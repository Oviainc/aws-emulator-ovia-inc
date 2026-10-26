// SPDX-License-Identifier: CC0-1.0

//! Where a `ValidationException`'s member path stops when the violation is inside a MAP.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 deserialization, the modelled-constraint walk and this service's refusal
//! renderer — so what is asserted is the message a signed AWS SDK request observes.
//!
//! The rule under test: a refusal names a member and the requirement it violated and
//! copies NOTHING the request sent, so the member path a clause states is built from
//! modelled member names and the walk's own structural steps alone — a map entry is
//! reported at the member that HOLDS it, with the requirement its value shape states,
//! never at the entry key the caller chose. `tests/all/error_message_rules.rs` spans the
//! scalar members; this suite spans the members whose entries are keyed by caller text.
//!
//! The rows differ along what the rule generalises over: the KEY (its characters, its
//! length, whether it is spelled like a member name or carries the path separator), the
//! VALUE constraint broken (length maximum, length minimum, character class, and two at
//! once), the number of broken entries in one map, the MEMBER (`tags`, whose entries are
//! strings; `chatbotNotificationChannel`, whose entries are lists of ARNs) and the
//! OPERATIONS that accept either (`CreateInvestigationGroup`, `UpdateInvestigationGroup`,
//! `TagResource`). Two control rows pin what the rule does NOT change: a scalar member
//! still reports under its own name, and a LIST element still reports its position,
//! which is a fact about the request's shape rather than anything it sent.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// A role ARN that satisfies the modelled `RoleArn` (length 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The requirements a tag VALUE states, as a clause bundles them at the map member.
///
/// From the map's value shape (`smithy_query(query="shape",shape="TagValue")`: `@length`
/// 1..256 and `@pattern` `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, reported without the anchors
/// a model spells a whole-string match with).
const TAG_VALUE_REQUIREMENTS: &[&str] = &[
    "Member must have length less than or equal to 256",
    "Member must have length greater than or equal to 1",
    r"Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*)",
];

/// The requirements an entry of `chatbotNotificationChannel` states, from the scalar
/// shape its value list holds (`smithy_query(query="shape",shape="ChatConfigurationArns")`:
/// the member targets `ChatConfigurationArn`, `@length` 20..2048, `@pattern` `^arn:.*$`).
const CHAT_ARN_REQUIREMENTS: &[&str] = &[
    "Member must have length less than or equal to 2048",
    "Member must have length greater than or equal to 20",
    "Member must satisfy regular expression pattern: arn:.*",
];

/// The longest key the modelled `TagKey` admits
/// (`smithy_query(query="shape",shape="TagKey")`: `@length` 1..128).
const MAX_TAG_KEY_LENGTH: usize = 128;

/// An SNS topic ARN a caller may key `chatbotNotificationChannel` by, satisfying the
/// modelled `SNSTopicArn` (length 20..2048, `^arn:.*$`).
const TOPIC_ARN: &str = "arn:aws:sns:us-west-2:000000000000:my-private-topic";

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

/// A request signed for `aiops` in [`REGION`], the signal the router routes on.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{REGION}/aiops/aws4_request, \
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

/// Status, error code and `message` of one call.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, String) {
    let response = router.dispatch(request(method, uri, body)).await;
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
    (status, code, message)
}

/// The whole message a map member's refusal states: the member, and the bundle of
/// requirements its value shape states.
fn map_message(member_path: &str, requirements: &[&str]) -> String {
    format!(
        "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
         Map value must satisfy constraint: [{}]",
        requirements.join(", ")
    )
}

/// The whole message a single non-map clause states.
fn member_message(member_path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// One row: the case, the request (method, URI, body), the caller-chosen text the
/// message may not carry, and the message the service must answer with.
struct Case {
    name: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    withheld: Vec<String>,
    expected: String,
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
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

/// A `CreateInvestigationGroup` body carrying `tags`.
fn create_with_tags(tags: Value) -> Value {
    json!({ "name": "map-paths", "roleArn": ROLE_ARN, "tags": tags })
}

/// The key shapes a caller may send, as `(case, key)`: ordinary text, a key carrying the
/// path separator, keys spelled like a modelled member and like the walk's own collection
/// step, non-ASCII text, and a key at the modelled `TagKey` maximum.
fn key_shapes() -> Vec<(&'static str, String)> {
    vec![
        ("an ordinary key", "CostCentre-4711".to_string()),
        ("a key carrying the path separator", "cost.centre".to_string()),
        ("a key spelled like the member", "tags".to_string()),
        ("a key spelled like the collection step", "member".to_string()),
        ("a key spelled like another member", "roleArn".to_string()),
        ("a non-ASCII key", "Ωmega.Проект".to_string()),
        (
            "a key at the modelled maximum",
            "k".repeat(MAX_TAG_KEY_LENGTH),
        ),
    ]
}

/// The value shapes that break a tag value's constraints, as
/// `(case, value, how many requirements it breaks at once)`.
fn broken_tag_values() -> Vec<(&'static str, String)> {
    vec![
        ("a value one character past the maximum", "v".repeat(257)),
        ("a value far past the maximum", "v".repeat(4096)),
        ("a value below the minimum", String::new()),
        ("a value outside the character class", "no*stars*here".to_string()),
        (
            "a value breaking the maximum and the class at once",
            "*".repeat(300),
        ),
    ]
}

#[tokio::test]
async fn a_refusal_inside_a_map_is_stated_at_the_map_member_and_never_at_the_entry_key() {
    let router = emulator();
    let mut cases: Vec<Case> = Vec::new();

    // Every (key shape x broken value) pair is one request, and all of them must state
    // the SAME clause: the member, and what its value shape requires.
    for (key_case, key) in key_shapes() {
        for (value_case, value) in broken_tag_values() {
            cases.push(Case {
                name: format!("tags with {key_case} and {value_case}"),
                method: "POST",
                uri: "/investigationGroups".to_string(),
                body: Some(create_with_tags(json!({ key.clone(): value.clone() }))),
                withheld: vec![key.clone(), value.clone()]
                    .into_iter()
                    .filter(|text| !text.is_empty())
                    .collect(),
                expected: map_message("tags", TAG_VALUE_REQUIREMENTS),
            });
        }
    }

    // Several broken entries in one map are one fact about one member, so the clause is
    // stated once however many entries broke it.
    cases.push(Case {
        name: "tags with three broken entries".to_string(),
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: Some(create_with_tags(json!({
            "first": "v".repeat(257),
            "second": "",
            "third": "no*stars*here",
        }))),
        withheld: vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
        ],
        expected: map_message("tags", TAG_VALUE_REQUIREMENTS),
    });

    // The same member on another operation: `TagResource` is served by the tagging
    // frontend and reaches the same boundary walk, so it states the same clause.
    cases.push(Case {
        name: "TagResource with a value past the maximum".to_string(),
        method: "POST",
        uri: format!("/tags/{}", label(ROLE_ARN)),
        body: Some(json!({ "tags": { "CostCentre-4711": "v".repeat(257) } })),
        withheld: vec!["CostCentre-4711".to_string()],
        expected: map_message("tags", TAG_VALUE_REQUIREMENTS),
    });

    // A map whose entries are LISTS: the key is a caller-supplied topic ARN and the
    // broken value sits one step further in, so the path the walk built carried the key
    // AND a position inside the entry. It is still stated at the member.
    for (case, arn) in [
        ("an entry ARN below its minimum", "arn:aws".to_string()),
        (
            "an entry outside its character class",
            "not-an-arn-but-long-enough".to_string(),
        ),
    ] {
        cases.push(Case {
            name: format!("chatbotNotificationChannel with {case}"),
            method: "POST",
            uri: "/investigationGroups".to_string(),
            body: Some(json!({
                "name": "map-paths",
                "roleArn": ROLE_ARN,
                "chatbotNotificationChannel": { TOPIC_ARN: [arn.clone()] },
            })),
            withheld: vec![TOPIC_ARN.to_string(), arn.clone()],
            expected: map_message("chatbotNotificationChannel", CHAT_ARN_REQUIREMENTS),
        });
        cases.push(Case {
            name: format!("UpdateInvestigationGroup's channel with {case}"),
            method: "PATCH",
            uri: "/investigationGroups/no-such-group".to_string(),
            body: Some(json!({
                "chatbotNotificationChannel": { TOPIC_ARN: ["arn:aws:chatbot::000000000000:chat-configuration/slack-channel/ops", arn.clone()] },
            })),
            withheld: vec![TOPIC_ARN.to_string(), arn.clone()],
            expected: map_message("chatbotNotificationChannel", CHAT_ARN_REQUIREMENTS),
        });
    }

    // Controls. A scalar member reports under its own name, and a list element keeps the
    // position the walk gave it: neither is anything the request sent.
    cases.push(Case {
        name: "a scalar member is unaffected".to_string(),
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: Some(json!({ "name": "map-paths", "roleArn": "arn:aws:iam" })),
        withheld: vec!["arn:aws:iam".to_string()],
        expected: member_message(
            "roleArn",
            "Member must have length greater than or equal to 20",
        ),
    });
    cases.push(Case {
        name: "a list element keeps its position".to_string(),
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: Some(json!({
            "name": "map-paths",
            "roleArn": ROLE_ARN,
            "crossAccountConfigurations": [
                { "sourceRoleArn": "arn:aws:iam::000000000001:role/Source" },
                { "sourceRoleArn": "arn:aws:iam" },
            ],
        })),
        withheld: vec!["arn:aws:iam::000000000001:role/Source".to_string()],
        expected: member_message(
            "crossAccountConfigurations.2.member.sourceRoleArn",
            "Member must have length greater than or equal to 20",
        ),
    });

    for case in cases {
        let Case {
            name,
            method,
            uri,
            body,
            withheld,
            expected,
        } = case;
        let (status, code, message) = call(&router, method, &uri, body.as_ref()).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} / {code} / {message:?}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {name:?}: expected ValidationException, got {code} / {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {name:?}: expected {expected:?}, got {message:?}"
        );
        for text in withheld {
            // A key spelled exactly like the member it sits under cannot be told from
            // the member name in the finished text, so the equality above is what pins
            // that row; everything the message is allowed to carry is skipped here.
            if expected.contains(&text) {
                continue;
            }
            assert!(
                !message.contains(&text),
                "case {name:?}: the message must not carry the caller's own text {text:?}, \
                 got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_map_entry_a_request_may_send_is_still_accepted_and_stored() {
    // The rule is about how a refusal is WORDED, so what the boundary admits must not
    // change: entries whose keys take the shapes above, with values the model allows, are
    // stored and read back through `GetInvestigationGroup`.
    let router = emulator();
    let mut tags = serde_json::Map::new();
    for (index, (_, key)) in key_shapes().into_iter().enumerate() {
        tags.insert(key, Value::String(format!("value-{index}")));
    }
    let expected: Vec<(String, Value)> = tags
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    let (status, code, message) = call(
        &router,
        "POST",
        "/investigationGroups",
        Some(&create_with_tags(Value::Object(tags))),
    )
    .await;
    assert_eq!(
        status, 201,
        "creating the group: expected 201, got {status} / {code} / {message:?}"
    );

    let response = router
        .dispatch(request("GET", "/investigationGroups", None))
        .await;
    let listed: Value = serde_json::from_slice(&response.into_body()).expect("a JSON body");
    let arn = listed
        .get("investigationGroups")
        .and_then(Value::as_array)
        .and_then(|groups| groups.first())
        .and_then(|group| group.get("arn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no created group in {listed:?}"))
        .to_string();

    let response = router
        .dispatch(request(
            "GET",
            &format!("/tags/{}", label(&arn)),
            None,
        ))
        .await;
    let status = response.status().as_u16();
    let body: Value = serde_json::from_slice(&response.into_body()).expect("a JSON body");
    assert_eq!(
        status, 200,
        "reading the tags back: expected 200, got {status} / {body:?}"
    );
    for (key, value) in expected {
        assert_eq!(
            body.get("tags").and_then(|tags| tags.get(&key)),
            Some(&value),
            "case {key:?}: expected the stored value {value:?}, got {body:?}"
        );
    }
}
