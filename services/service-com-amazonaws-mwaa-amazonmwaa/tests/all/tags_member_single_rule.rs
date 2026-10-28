// SPDX-License-Identifier: CC0-1.0

//! Every response that carries an environment's tags decides the `Tags` member the SAME
//! way, end to end over HTTP through the router this service mounts in production.
//!
//! The rule each case asserts: a stored environment's tag map becomes the modelled `Tags`
//! member of every response reporting it — the member is absent exactly when the map is
//! empty (`TagMap` is `@length(1, 50)`, so an empty map lies outside its own bound), and
//! otherwise it is the stored map pair for pair — so `ListTagsForResource` and the
//! `Environment` shape `GetEnvironment` returns can never disagree about what an
//! environment's tags are.
//!
//! Each table row varies what the rule generalises over: how many pairs the map holds
//! (none, one, a handful, the full `TagMap`-sized map), how long the keys and values are
//! (one character, the longest `TagKey` and `TagValue` admit), and which characters they
//! are spelled with (ASCII, a non-ASCII letter, the separator punctuation the pattern
//! allows). Every row is then read through BOTH operations after a create seeded the map,
//! after a `TagResource` grew it and after an `UntagResource` emptied it, so the two
//! responses are compared on a non-empty map and on an empty one alike.

use std::collections::BTreeMap;
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

    /// Create one environment, seeding the given tag map when it has any pair.
    async fn create(&self, name: &str, tags: &BTreeMap<String, String>) -> Answer {
        let mut body = valid_create_body();
        if let Some(tags) = tag_map_json(tags) {
            body.as_object_mut()
                .expect("the create body is an object")
                .insert("Tags".to_string(), tags);
        }
        self.send("PUT", &format!("/environments/{name}"), Some(body))
            .await
    }

    /// The `Tags` member `GetEnvironment` reports for `name`, or `None` when it reports
    /// none.
    async fn environment_tags(&self, name: &str) -> Option<Value> {
        let answer = self.send("GET", &format!("/environments/{name}"), None).await;
        assert_eq!(
            answer.status,
            200,
            "GetEnvironment({name}) must succeed, got {} {}",
            answer.status,
            answer.text()
        );
        answer
            .json()
            .get("Environment")
            .and_then(|environment| environment.get("Tags"))
            .cloned()
    }

    /// The `Tags` member `ListTagsForResource` reports for `arn`, or `None` when it
    /// reports none.
    async fn listed_tags(&self, arn: &str) -> Option<Value> {
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
        answer.json().get("Tags").cloned()
    }

    async fn tag(&self, arn: &str, tags: Value) -> Answer {
        self.send(
            "POST",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({"Tags": tags})),
        )
        .await
    }

    async fn untag(&self, arn: &str, keys: &[String]) -> Answer {
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

/// Percent-encode one `tagKeys` query value the way an SDK encodes an `@httpQuery`
/// member: every byte outside RFC 3986's unreserved set is escaped, which is always a
/// legal encoding of the value.
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

/// The modelled `Tags` member for a tag map: `None` when the map is empty, because
/// `TagMap` is `@length(1, 50)`. This is the expectation BOTH responses are held to.
fn tag_map_json(tags: &BTreeMap<String, String>) -> Option<Value> {
    (!tags.is_empty()).then(|| {
        Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        )
    })
}

/// One stored tag map, as a row of the table below.
struct TagCase {
    case: &'static str,
    name: &'static str,
    tags: Vec<(String, String)>,
}

/// The tag maps the rule is asserted over. The counts come from `TagMap`'s own
/// `@length(1, 50)` and the key/value sizes from `TagKey`'s `@length(1, 128)` and
/// `TagValue`'s `@length(1, 256)`
/// (`smithy_query(query="shape",shape="TagMap,TagKey,TagValue",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`);
/// the non-ASCII and punctuation rows exercise the `\p{L}` and `_.:/=+-@` classes the
/// shared key/value pattern allows.
fn cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "no tags at all",
            name: "tagsNone",
            tags: Vec::new(),
        },
        TagCase {
            case: "a single pair",
            name: "tagsOne",
            tags: vec![("Environment".to_string(), "Staging".to_string())],
        },
        TagCase {
            case: "a handful of pairs",
            name: "tagsFew",
            tags: vec![
                ("Environment".to_string(), "Production".to_string()),
                ("Owner".to_string(), "data platform".to_string()),
                ("CostCentre".to_string(), "4711".to_string()),
            ],
        },
        TagCase {
            case: "the shortest key and value either shape admits",
            name: "tagsShortest",
            tags: vec![("k".to_string(), "v".to_string())],
        },
        TagCase {
            case: "the longest key and value either shape admits",
            name: "tagsLongest",
            tags: vec![("K".repeat(128), "V".repeat(256))],
        },
        TagCase {
            case: "non-ASCII letters in the key and the value",
            name: "tagsUnicode",
            tags: vec![("Mitarbeiterin".to_string(), "Zürich".to_string())],
        },
        TagCase {
            case: "the separator punctuation the pattern allows",
            name: "tagsPunctuation",
            tags: vec![(
                "team.group:sub/unit=one+two-three@four".to_string(),
                "value.with:every/allowed=char+set-@".to_string(),
            )],
        },
        TagCase {
            case: "a full TagMap-sized map",
            name: "tagsFull",
            tags: (0..state::MAX_TAGS)
                .map(|index| (format!("key{index:02}"), format!("value{index:02}")))
                .collect(),
        },
    ]
}

impl TagCase {
    fn arn(&self) -> String {
        format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{}", self.name)
    }

    fn stored(&self) -> BTreeMap<String, String> {
        self.tags.iter().cloned().collect()
    }
}

/// Both responses carrying an environment's tags report the SAME `Tags` member, over a
/// map seeded by the create, grown by `TagResource` and emptied by `UntagResource`.
#[tokio::test]
async fn list_tags_and_get_environment_report_one_tags_member() {
    let harness = Harness::new();

    for case in cases() {
        let mut expected = case.stored();
        let created = harness.create(case.name, &expected).await;
        assert_eq!(
            created.status,
            200,
            "case {}: CreateEnvironment must succeed, got {} {}",
            case.case,
            created.status,
            created.text()
        );
        let arn = case.arn();

        // Seeded by the create: both responses report the stored map.
        let listed = harness.listed_tags(&arn).await;
        let described = harness.environment_tags(case.name).await;
        assert_eq!(
            listed,
            tag_map_json(&expected),
            "case {}: ListTagsForResource must report the tags the create stored, got {listed:?}",
            case.case
        );
        assert_eq!(
            described, listed,
            "case {}: GetEnvironment must report the same Tags member as ListTagsForResource, got {described:?}",
            case.case
        );

        // Grown by a write: a map that was empty becomes non-empty, and one that was
        // full enough stays the map both responses read. `TagMap` caps an environment at
        // `MAX_TAGS` pairs, so the full row has its own key overwritten instead.
        let (added_key, added_value) = if expected.len() < state::MAX_TAGS {
            ("AddedByTagResource".to_string(), "written".to_string())
        } else {
            let key = expected
                .keys()
                .next()
                .expect("a full map has a first key")
                .clone();
            (key, "overwritten".to_string())
        };
        let written = Value::Object(
            [(added_key.clone(), Value::String(added_value.clone()))]
                .into_iter()
                .collect(),
        );
        let tagged = harness.tag(&arn, written).await;
        assert_eq!(
            tagged.status,
            200,
            "case {}: TagResource must succeed, got {} {}",
            case.case,
            tagged.status,
            tagged.text()
        );
        expected.insert(added_key, added_value);
        let listed = harness.listed_tags(&arn).await;
        let described = harness.environment_tags(case.name).await;
        assert_eq!(
            listed,
            tag_map_json(&expected),
            "case {}: ListTagsForResource must report the map TagResource wrote, got {listed:?}",
            case.case
        );
        assert_eq!(
            described, listed,
            "case {}: GetEnvironment must agree after TagResource, got {described:?}",
            case.case
        );

        // Emptied: an empty map lies outside `TagMap`'s own bound, so NEITHER response
        // reports a `Tags` member.
        let keys: Vec<String> = expected.keys().cloned().collect();
        let untagged = harness.untag(&arn, &keys).await;
        assert_eq!(
            untagged.status,
            200,
            "case {}: UntagResource must succeed, got {} {}",
            case.case,
            untagged.status,
            untagged.text()
        );
        let listed = harness.listed_tags(&arn).await;
        let described = harness.environment_tags(case.name).await;
        assert_eq!(
            listed, None,
            "case {}: an untagged environment must report no Tags member, got {listed:?}",
            case.case
        );
        assert_eq!(
            described, None,
            "case {}: GetEnvironment must agree the environment is untagged, got {described:?}",
            case.case
        );
    }
}
