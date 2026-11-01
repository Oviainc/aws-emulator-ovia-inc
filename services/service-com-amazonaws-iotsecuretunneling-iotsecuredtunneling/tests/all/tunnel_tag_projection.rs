// SPDX-License-Identifier: CC0-1.0

//! How a reported tunnel carries its tag set.
//!
//! RULE: the `tags` member of a reported tunnel always carries the tunnel's
//! stored tag set as a JSON array — the array is EMPTY when the tunnel carries
//! no tags, and the member is never dropped. A reader can therefore iterate the
//! member without having to tell "this tunnel has no tags" from "its tags were
//! not reported", and the tag store round-trips: removing a tunnel's last tag
//! leaves the member as `[]` instead of making it disappear.
//!
//! Rows differ along what the rule generalises over: how many tags the tunnel
//! carries (none, one, several), the form of the keys and values (an empty
//! value, which `Tag$value`'s `length(min: 0, max: 256)` allows, and the
//! punctuation `TagKey`'s pattern allows — both from
//! `smithy_query(query="shape",shape="Tag",model_path="services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/smithy-model/com-amazonaws-iotsecuretunneling-iotsecuredtunneling.json")`),
//! how the reported set came about (set at open time, written afterwards,
//! partly removed, wholly removed, removed and written again, or a removal that
//! stored nothing), and the status of the tunnel being reported (`OPEN` and
//! `CLOSED`).
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-east-2";

/// A router carrying only this service.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// What happens to the tunnel after it is opened, before it is reported.
#[derive(Debug, Clone)]
enum Step {
    /// A `TagResource` writing these `(key, value)` pairs.
    Tag(Vec<(&'static str, &'static str)>),
    /// An `UntagResource` removing these keys.
    Untag(Vec<&'static str>),
    /// An explicit `CloseTunnel`, which leaves the record readable.
    Close,
}

/// One row: a tunnel built some way, and the tag set its report must carry.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    /// The `tags` member `OpenTunnel` is sent, or `None` to omit it.
    open_tags: Option<Vec<(&'static str, &'static str)>>,
    steps: Vec<Step>,
    expected: Vec<(&'static str, &'static str)>,
}

/// A `tags` list as a request carries it.
fn tags_member(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// `pairs` as the set they denote, so a row states its expectation without
/// committing to an order.
fn expected_set(pairs: &[(&'static str, &'static str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn a_reported_tunnel_always_carries_its_tag_set_as_a_list() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let rows: Vec<Case> = vec![
        Case {
            label: "opened with no tags member at all",
            open_tags: None,
            steps: vec![],
            expected: vec![],
        },
        Case {
            label: "opened with one tag",
            open_tags: Some(vec![("env", "test")]),
            steps: vec![],
            expected: vec![("env", "test")],
        },
        Case {
            label: "opened with several tags, including an empty value and \
                    punctuation the key pattern allows",
            open_tags: Some(vec![
                ("env", "prod"),
                ("owner", "alice"),
                ("cost.center/42", "finance=eu"),
                ("blank", ""),
            ]),
            steps: vec![],
            expected: vec![
                ("env", "prod"),
                ("owner", "alice"),
                ("cost.center/42", "finance=eu"),
                ("blank", ""),
            ],
        },
        Case {
            label: "untagged at open, tagged afterwards",
            open_tags: None,
            steps: vec![Step::Tag(vec![("team", "iot")])],
            expected: vec![("team", "iot")],
        },
        Case {
            label: "one key of two removed",
            open_tags: Some(vec![("env", "test"), ("owner", "bob")]),
            steps: vec![Step::Untag(vec!["env"])],
            expected: vec![("owner", "bob")],
        },
        Case {
            label: "every key removed, so the set is empty again",
            open_tags: Some(vec![("env", "test"), ("owner", "bob")]),
            steps: vec![Step::Untag(vec!["env", "owner"])],
            expected: vec![],
        },
        Case {
            label: "every key removed and then a fresh key written",
            open_tags: Some(vec![("env", "test")]),
            steps: vec![
                Step::Untag(vec!["env"]),
                Step::Tag(vec![("fresh", "value")]),
            ],
            expected: vec![("fresh", "value")],
        },
        Case {
            label: "a removal that names keys an untagged tunnel never carried",
            open_tags: None,
            steps: vec![Step::Untag(vec!["absent", "also-absent"])],
            expected: vec![],
        },
        Case {
            label: "a closed tunnel with no tags",
            open_tags: None,
            steps: vec![Step::Close],
            expected: vec![],
        },
        Case {
            label: "a closed tunnel whose tags were all removed first",
            open_tags: Some(vec![("env", "test")]),
            steps: vec![Step::Untag(vec!["env"]), Step::Close],
            expected: vec![],
        },
        Case {
            label: "a closed tunnel that still carries its tags",
            open_tags: Some(vec![("env", "test")]),
            steps: vec![Step::Close],
            expected: vec![("env", "test")],
        },
    ];

    for case in rows {
        let label = case.label;
        let mut input = json!({
            "description": "tag projection",
            "destinationConfig": {"thingName": "thing-tag-projection", "services": ["SSH"]},
        });
        if let Some(open_tags) = case.open_tags.as_ref() {
            input["tags"] = tags_member(open_tags);
        }
        let (status, opened) = call(&router, "OpenTunnel", &input).await;
        assert_eq!(
            status, 200,
            "case {label:?}: OpenTunnel answered {status} {opened:?}"
        );
        let tunnel_id = opened["tunnelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no tunnelId in {opened:?}"))
            .to_string();
        let tunnel_arn = opened["tunnelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no tunnelArn in {opened:?}"))
            .to_string();

        for step in &case.steps {
            let (operation, step_input) = match step {
                Step::Tag(pairs) => (
                    "TagResource",
                    json!({"resourceArn": &tunnel_arn, "tags": tags_member(pairs)}),
                ),
                Step::Untag(keys) => (
                    "UntagResource",
                    json!({"resourceArn": &tunnel_arn, "tagKeys": keys}),
                ),
                Step::Close => ("CloseTunnel", json!({"tunnelId": &tunnel_id})),
            };
            let (status, body) = call(&router, operation, &step_input).await;
            assert_eq!(
                status, 200,
                "case {label:?}: {operation} {step:?} answered {status} {body:?}"
            );
        }

        let (status, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": &tunnel_id})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeTunnel answered {status} {described:?}"
        );
        let reported = described["tunnel"].get("tags");

        // The member is always there, and it is always a list.
        let list = match reported {
            Some(Value::Array(entries)) => entries.clone(),
            other => panic!(
                "case {label:?}: expected tunnel.tags to be a JSON array, got {other:?}"
            ),
        };
        // Every entry is a modelled Tag, and the set is exactly what is stored.
        let actual: BTreeMap<String, String> = list
            .iter()
            .map(|entry| {
                let key = entry["key"].as_str().unwrap_or_else(|| {
                    panic!("case {label:?}: a tag entry carries no key: {entry:?}")
                });
                let value = entry["value"].as_str().unwrap_or_else(|| {
                    panic!("case {label:?}: a tag entry carries no value: {entry:?}")
                });
                (key.to_string(), value.to_string())
            })
            .collect();
        let expected = expected_set(&case.expected);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected tunnel.tags to report {expected:?}, got {list:?}"
        );
        assert_eq!(
            list.len(),
            expected.len(),
            "case {label:?}: expected {} tag entries, got {list:?}",
            expected.len()
        );
        if expected.is_empty() {
            assert!(
                list.is_empty(),
                "case {label:?}: an untagged tunnel reports an empty list, got {list:?}"
            );
        }
    }
}

/// The empty list has to survive serialization, not just construction: assert it
/// on the bytes the service answers with, where an omitted member and an empty
/// list are plainly different.
#[tokio::test]
async fn an_untagged_tunnel_serializes_tags_as_an_empty_array() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // Each row reaches the untagged state a different way; all three must answer
    // with the member present and empty.
    let rows: [&str; 3] = [
        "never tagged",
        "tagged then untagged",
        "tagged, untagged, untagged again",
    ];

    for label in rows {
        let (status, opened) = call(
            &router,
            "OpenTunnel",
            &json!({
                "destinationConfig": {"thingName": "thing-empty-tags", "services": ["SSH"]},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: OpenTunnel answered {status} {opened:?}"
        );
        let tunnel_id = opened["tunnelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no tunnelId in {opened:?}"))
            .to_string();
        let tunnel_arn = opened["tunnelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no tunnelArn in {opened:?}"))
            .to_string();

        if label != "never tagged" {
            let (status, body) = call(
                &router,
                "TagResource",
                &json!({
                    "resourceArn": &tunnel_arn,
                    "tags": [{"key": "env", "value": "test"}],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: TagResource answered {status} {body:?}"
            );
            let removals = if label == "tagged then untagged" { 1 } else { 2 };
            for _ in 0..removals {
                let (status, body) = call(
                    &router,
                    "UntagResource",
                    &json!({"resourceArn": &tunnel_arn, "tagKeys": ["env"]}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: UntagResource answered {status} {body:?}"
                );
            }
        }

        let (status, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": &tunnel_id})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeTunnel answered {status} {described:?}"
        );
        let serialized = serde_json::to_string(&described).expect("re-serialize the response");
        assert!(
            serialized.contains("\"tags\":[]"),
            "case {label:?}: expected the response to carry \"tags\":[], got {serialized}"
        );
    }
}
