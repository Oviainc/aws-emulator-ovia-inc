// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, end to end over HTTP through the router this service
//! mounts in production.
//!
//! The rule every case here asserts: a tag whose key opens with the prefix Amazon Web
//! Services reserves for its own tags (`state::RESERVED_TAG_KEY_PREFIX`, matched whatever
//! the case of the prefix) is not the caller's to write OR to remove, so every operation
//! that writes this service's tag map — `CreateEnvironment`'s `Tags`, `TagResource` and
//! `UntagResource` — refuses a request naming one with HTTP 400 `ValidationException` and
//! changes nothing.
//!
//! Each table row varies what the rule generalises over: what follows the prefix (nothing,
//! one character, a dotted/colon-separated system key, a non-ASCII letter, the length
//! `TagKey` allows), the case the prefix is spelled in, how many keys the request carries
//! and which operation carries them. The complementary table pins the other side of the
//! prefix test: a key that merely STARTS WITH `aws` without the colon, or carries the
//! prefix somewhere other than the start, is an ordinary caller-owned key and round-trips
//! through all three operations.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region the `AUTHORIZATION` scope above signs for.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    /// Create one environment, optionally with a `Tags` map.
    async fn create(&self, name: &str, tags: Option<Value>) -> Answer {
        let mut body = valid_create_body();
        if let Some(tags) = tags {
            body.as_object_mut()
                .expect("the create body is an object")
                .insert("Tags".to_string(), tags);
        }
        self.send("PUT", &format!("/environments/{name}"), Some(body))
            .await
    }

    async fn get(&self, name: &str) -> Answer {
        self.send("GET", &format!("/environments/{name}"), None).await
    }

    async fn tag(&self, arn: &str, tags: Value) -> Answer {
        self.send(
            "POST",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({"Tags": tags})),
        )
        .await
    }

    async fn untag(&self, arn: &str, keys: &[&str]) -> Answer {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={}", encode_query(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.send(
            "DELETE",
            &format!("/tags/{}?{query}", encode_label(arn)),
            None,
        )
        .await
    }

    /// The `Tags` map `ListTagsForResource` reports, or `Value::Null` when it reports none.
    async fn tags(&self, arn: &str) -> Value {
        let answer = self
            .send("GET", &format!("/tags/{}", encode_label(arn)), None)
            .await;
        assert_eq!(
            answer.status,
            200,
            "ListTagsForResource({arn}) must succeed, got {} {}",
            answer.status,
            answer.text()
        );
        answer.json().get("Tags").cloned().unwrap_or(Value::Null)
    }
}

/// A create body every member of which satisfies its modelled constraints.
fn valid_create_body() -> Value {
    json!({
        "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
        "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
        "DagS3Path": "dags",
        "NetworkConfiguration": {
            "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            "SecurityGroupIds": ["sg-00000000000000001"],
        },
    })
}

/// Percent-encode the characters an ARN carries that are not legal in a single
/// (non-greedy) `@httpLabel` segment — exactly what an SDK does for `ResourceArn`.
fn encode_label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// Percent-encode one `tagKeys` query value, the way an SDK encodes an `@httpQuery`
/// member: `TagKey` admits `:` and any Unicode letter, neither of which can go into a
/// query string raw. Every byte outside RFC 3986's unreserved set is escaped, which is
/// always a legal encoding of the value.
fn encode_query(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A `TagMap` JSON object of the given pairs. Built rather than written as a `json!`
/// literal because the keys are the table's own values.
fn tag_map(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

/// [`state::RESERVED_TAG_KEY_PREFIX`] with its first letter capitalised, for the row that
/// spells the prefix in mixed case.
fn capitalised_prefix() -> String {
    let mut chars = state::RESERVED_TAG_KEY_PREFIX.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// One tag key, and whether it lies in the reserved namespace.
struct KeyCase {
    case: &'static str,
    key: String,
}

/// Keys in the reserved namespace. The prefix itself is
/// [`state::RESERVED_TAG_KEY_PREFIX`]; the rows vary what follows it and the case it is
/// spelled in. The system keys quoted are ones AWS itself applies (the CloudFormation
/// stack tags and `aws:created-by`); the long row is the longest key `TagKey`'s own
/// `@length(1, 128)` admits
/// (`smithy_query(query="shape",shape="TagKey",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`),
/// and the non-ASCII row exercises the `\p{L}` class that pattern allows.
fn reserved_keys() -> Vec<KeyCase> {
    let prefix = state::RESERVED_TAG_KEY_PREFIX;
    vec![
        KeyCase {
            case: "the reserved prefix alone",
            key: prefix.to_string(),
        },
        KeyCase {
            case: "one character after the prefix",
            key: format!("{prefix}x"),
        },
        KeyCase {
            case: "a CloudFormation stack tag",
            key: format!("{prefix}cloudformation:stack-name"),
        },
        KeyCase {
            case: "a provisioning tag",
            key: format!("{prefix}created-by"),
        },
        KeyCase {
            case: "the prefix in upper case",
            key: format!("{}billing", prefix.to_uppercase()),
        },
        KeyCase {
            case: "the prefix in mixed case",
            key: format!("{}Owner", capitalised_prefix()),
        },
        KeyCase {
            case: "a non-ASCII letter after the prefix",
            key: format!("{prefix}Проект"),
        },
        KeyCase {
            case: "the longest key TagKey admits",
            key: format!("{prefix}{}", "k".repeat(128 - prefix.chars().count())),
        },
    ]
}

/// Keys that are NOT reserved, because the rule is a PREFIX test: the namespace marker is
/// `aws` FOLLOWED BY the colon, at the START of the key. Each row would be caught by a
/// looser test (a substring search, an `aws`-without-colon test, a case-blind contains)
/// and must be accepted.
fn ordinary_keys() -> Vec<KeyCase> {
    let bare = state::RESERVED_TAG_KEY_PREFIX.trim_end_matches(':').to_string();
    vec![
        KeyCase {
            case: "the prefix without its colon",
            key: bare.clone(),
        },
        KeyCase {
            case: "a word opening with the prefix letters",
            key: format!("{bare}ome:team"),
        },
        KeyCase {
            case: "the prefix in upper case without its colon",
            key: format!("{}Region", bare.to_uppercase()),
        },
        KeyCase {
            case: "the prefix carried later in the key",
            key: format!("team:{}:region", state::RESERVED_TAG_KEY_PREFIX.trim_end_matches(':')),
        },
        KeyCase {
            case: "the shortest key TagKey admits",
            key: "a".to_string(),
        },
    ]
}

/// The ARN of the environment `name`, as every emulated ARN in this Region spells it.
fn environment_arn(name: &str) -> String {
    format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{name}")
}

/// The tags the fixture environment is seeded with: ordinary keys, so every refusal below
/// is judged against a tag map that does hold something.
fn seeded_tags() -> Value {
    tag_map(&[("team", "airflow"), ("owner", "sre")])
}

#[tokio::test]
async fn the_rows_agree_with_the_rule_the_service_enforces() {
    for row in reserved_keys() {
        let case = row.case;
        assert!(
            state::is_reserved_tag_key(&row.key),
            "case {case:?}: {:?} must be in the reserved namespace",
            row.key
        );
    }
    for row in ordinary_keys() {
        let case = row.case;
        assert!(
            !state::is_reserved_tag_key(&row.key),
            "case {case:?}: {:?} must NOT be in the reserved namespace",
            row.key
        );
    }
}

#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_key_and_writes_nothing() {
    let harness = Harness::new();
    assert_eq!(
        harness.create("TagReserved", Some(seeded_tags())).await.status,
        200,
        "the fixture create must succeed"
    );
    let arn = environment_arn("TagReserved");

    for row in reserved_keys() {
        let case = row.case;
        let answer = harness.tag(&arn, tag_map(&[(row.key.as_str(), "v")])).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: TagResource({:?}) must be refused, got {} {}",
            row.key,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert!(
            answer.message().contains("Tags") && answer.message().contains(&row.key),
            "case {case:?}: the refusal must name the member and the key, got {:?}",
            answer.message()
        );
        assert_eq!(
            harness.tags(&arn).await,
            seeded_tags(),
            "case {case:?}: the stored tag map must be untouched"
        );
    }

    // A request that mixes an ordinary key with a reserved one is refused WHOLE: the
    // ordinary pair is not written either.
    let mixed = harness
        .tag(
            &arn,
            tag_map(&[
                ("cost-centre", "42"),
                (&format!("{}internal", state::RESERVED_TAG_KEY_PREFIX), "v"),
            ]),
        )
        .await;
    assert_eq!(
        mixed.status,
        400,
        "case \"an ordinary key beside a reserved one\": expected 400, got {} {}",
        mixed.status,
        mixed.text()
    );
    assert_eq!(
        harness.tags(&arn).await,
        seeded_tags(),
        "case \"an ordinary key beside a reserved one\": no pair of the refused request may be written"
    );
}

#[tokio::test]
async fn untag_resource_refuses_a_reserved_tag_key_and_removes_nothing() {
    let harness = Harness::new();
    assert_eq!(
        harness
            .create("UntagReserved", Some(seeded_tags()))
            .await
            .status,
        200,
        "the fixture create must succeed"
    );
    let arn = environment_arn("UntagReserved");

    for row in reserved_keys() {
        let case = row.case;
        let answer = harness.untag(&arn, &[row.key.as_str()]).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: UntagResource({:?}) must be refused, got {} {}",
            row.key,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert!(
            answer.message().contains("tagKeys") && answer.message().contains(&row.key),
            "case {case:?}: the refusal must name the member and the key, got {:?}",
            answer.message()
        );
        assert_eq!(
            harness.tags(&arn).await,
            seeded_tags(),
            "case {case:?}: the stored tag map must be untouched"
        );
    }

    // An ordinary key named BESIDE a reserved one is not removed either: the request is
    // refused whole.
    let reserved = format!("{}internal", state::RESERVED_TAG_KEY_PREFIX);
    let mixed = harness.untag(&arn, &["team", reserved.as_str()]).await;
    assert_eq!(
        mixed.status,
        400,
        "case \"an ordinary key beside a reserved one\": expected 400, got {} {}",
        mixed.status,
        mixed.text()
    );
    assert_eq!(
        harness.tags(&arn).await,
        seeded_tags(),
        "case \"an ordinary key beside a reserved one\": no key of the refused request may be removed"
    );
}

#[tokio::test]
async fn create_environment_refuses_a_reserved_tag_key_and_records_no_environment() {
    let harness = Harness::new();

    for (index, row) in reserved_keys().into_iter().enumerate() {
        let case = row.case;
        let name = format!("CreateReserved{index}");
        let answer = harness
            .create(&name, Some(tag_map(&[(row.key.as_str(), "v")])))
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: CreateEnvironment with {:?} must be refused, got {} {}",
            row.key,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        let read = harness.get(&name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must record no environment, got {} {}",
            read.status,
            read.text()
        );
    }
}

#[tokio::test]
async fn an_ordinary_key_that_resembles_the_reserved_prefix_round_trips() {
    let harness = Harness::new();

    for (index, row) in ordinary_keys().into_iter().enumerate() {
        let case = row.case;
        let name = format!("Ordinary{index}");
        // The create direction: the key goes in through `CreateEnvironment`'s own map and
        // reads back through the implemented `GetEnvironment`.
        let created = harness
            .create(&name, Some(tag_map(&[(row.key.as_str(), "value")])))
            .await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: CreateEnvironment with {:?} must be accepted, got {} {}",
            row.key,
            created.status,
            created.text()
        );
        let read = harness.get(&name).await;
        assert_eq!(
            read.json()
                .get("Environment")
                .and_then(|environment| environment.get("Tags"))
                .cloned(),
            Some(tag_map(&[(row.key.as_str(), "value")])),
            "case {case:?}: GetEnvironment must report the stored tag, got {}",
            read.text()
        );

        // The write direction, then the delete direction, through the tagging operations.
        let arn = environment_arn(&name);
        let tagged = harness
            .tag(&arn, tag_map(&[(row.key.as_str(), "written")]))
            .await;
        assert_eq!(
            tagged.status,
            200,
            "case {case:?}: TagResource({:?}) must be accepted, got {} {}",
            row.key,
            tagged.status,
            tagged.text()
        );
        assert_eq!(
            harness.tags(&arn).await,
            tag_map(&[(row.key.as_str(), "written")]),
            "case {case:?}: the written value must read back"
        );
        let untagged = harness.untag(&arn, &[row.key.as_str()]).await;
        assert_eq!(
            untagged.status,
            200,
            "case {case:?}: UntagResource({:?}) must be accepted, got {} {}",
            row.key,
            untagged.status,
            untagged.text()
        );
        assert_eq!(
            harness.tags(&arn).await,
            Value::Null,
            "case {case:?}: the removed key must be gone"
        );
    }
}
