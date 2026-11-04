// SPDX-License-Identifier: CC0-1.0

//! The per-tunnel tag-limit answer is the SAME answer at both tagging entry
//! points, byte for byte.
//!
//! RULE: whether a request is within the per-tunnel tag limit depends only on
//! the number of distinct tag keys the tunnel would end up carrying against
//! `state::MAX_TAGS_PER_TUNNEL` — never on which operation carried the keys nor
//! on how the count was reached — so `OpenTunnel`'s `tags` and `TagResource`'s
//! `tags` answer one merged count with the same HTTP status, the same error code
//! and the same message, and a refusal at either entry point stores nothing.
//!
//! Rows differ along what the rule generalises over: the merged key count (at the
//! limit, each side of it, far past it, and up to `TagList`'s modelled maximum of
//! 200 entries, from
//! `smithy_query(query="shape", shape="com.amazonaws.iotsecuretunneling#TagList")`)
//! and how that count is reached (all in one request, or stored keys plus new
//! ones). The limit itself is read from the crate rather than re-spelled here.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::MAX_TAGS_PER_TUNNEL;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// A router carrying only this service, over the engine the test also inspects.
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

/// A modelled `TagList` naming `keys`, each with a value.
fn tag_list(keys: &[String]) -> Value {
    Value::Array(
        keys.iter()
            .map(|key| json!({"key": key, "value": "v"}))
            .collect(),
    )
}

/// `prefix0..prefix{count-1}`.
fn distinct_keys(count: usize, prefix: &str) -> Vec<String> {
    (0..count).map(|index| format!("{prefix}{index}")).collect()
}

/// The keys the tag store holds for `arn`, read back through
/// `ListTagsForResource`.
async fn stored_keys(router: &MultiServiceRouter, arn: &str) -> BTreeSet<String> {
    let (status, listed) = call(router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource({arn}) answered {status} {listed:?}"
    );
    listed
        .get("tags")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|tag| Some(tag["key"].as_str()?.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Open a tunnel carrying `keys` (none when the slice is empty) and return
/// `(status, body)`.
async fn open_with(router: &MultiServiceRouter, case: &str, keys: &[String]) -> (u16, Value) {
    let mut input = json!({
        "description": case,
        "destinationConfig": {"thingName": "thing-shared-tag-limit", "services": ["SSH"]},
    });
    if !keys.is_empty() {
        input["tags"] = tag_list(keys);
    }
    call(router, "OpenTunnel", &input).await
}

/// The error code (`__type` on awsJson) and message of a refusal body, as the
/// pair the two entry points must agree on.
fn refusal(body: &Value) -> (Option<&str>, Option<&str>) {
    (body["__type"].as_str(), body["message"].as_str())
}

/// One row: how many keys the tunnel already carries when the tagging call
/// arrives, and how many NEW distinct keys that call names. The merged count —
/// what the rule is about — is their sum.
#[derive(Debug, Clone, Copy)]
struct Case {
    seeded: usize,
    fresh: usize,
}

impl Case {
    /// The distinct keys the tunnel would end up carrying.
    fn merged(self) -> usize {
        self.seeded + self.fresh
    }
}

#[tokio::test]
async fn both_tagging_entry_points_answer_one_merged_tag_count_identically() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let limit = MAX_TAGS_PER_TUNNEL;
    // Rows: each side of the limit, the limit itself reached two different ways,
    // and overshoots up to the modelled `TagList` maximum of 200 entries.
    let cases: [Case; 7] = [
        Case {
            seeded: 0,
            fresh: limit - 1,
        },
        Case {
            seeded: 0,
            fresh: limit,
        },
        Case {
            seeded: limit - 1,
            fresh: 1,
        },
        Case {
            seeded: limit,
            fresh: 1,
        },
        Case {
            seeded: 0,
            fresh: limit + 1,
        },
        Case {
            seeded: limit - 10,
            fresh: 20,
        },
        Case {
            seeded: 0,
            fresh: 200,
        },
    ];

    for case in cases {
        let merged = case.merged();
        let within = merged <= limit;
        let seeded = distinct_keys(case.seeded, "seed");
        let fresh = distinct_keys(case.fresh, "fresh");
        // Every distinct key of the row, which is what the merged count counts.
        let all: Vec<String> = seeded.iter().chain(fresh.iter()).cloned().collect();

        // Entry point 1: every key arrives with `OpenTunnel`.
        let label = format!("{merged} merged keys via OpenTunnel");
        let (open_status, open_body) = open_with(&router, &label, &all).await;
        if within {
            assert_eq!(
                open_status, 200,
                "case {label:?}: {merged} distinct keys is within the limit of {limit}, answered \
                 {open_status} {open_body:?}"
            );
            let arn = open_body["tunnelArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let stored = stored_keys(&router, &arn).await;
            assert_eq!(
                stored.len(),
                merged,
                "case {label:?}: the tunnel must carry the {merged} keys it was opened with, got \
                 {stored:?}"
            );
        } else {
            assert_eq!(
                open_status, 400,
                "case {label:?}: {merged} distinct keys is past the limit of {limit}, answered \
                 {open_status} {open_body:?}"
            );
            for field in ["tunnelId", "tunnelArn", "sourceAccessToken"] {
                assert!(
                    open_body.get(field).is_none(),
                    "case {label:?}: a refused open must hand out no {field}, got {:?}",
                    open_body.get(field)
                );
            }
        }

        // Entry point 2: the same merged count, reached through `TagResource` on
        // a tunnel already carrying this row's seeded keys — so the only
        // difference from entry point 1 is which call carried the keys.
        let label = format!(
            "{merged} merged keys via TagResource on a tunnel holding {} keys",
            case.seeded
        );
        let (status, opened) = open_with(&router, &label, &seeded).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the seed tunnel must open, answered {status} {opened:?}"
        );
        let arn = opened["tunnelArn"].as_str().unwrap_or_default().to_string();
        let (tag_status, tag_body) = call(
            &router,
            "TagResource",
            &json!({"resourceArn": arn, "tags": tag_list(&fresh)}),
        )
        .await;
        let stored = stored_keys(&router, &arn).await;
        if within {
            assert_eq!(
                tag_status, 200,
                "case {label:?}: {merged} distinct keys is within the limit of {limit}, answered \
                 {tag_status} {tag_body:?}"
            );
            assert_eq!(
                stored.len(),
                merged,
                "case {label:?}: the tunnel must end up carrying {merged} keys, got {stored:?}"
            );
        } else {
            assert_eq!(
                tag_status, 400,
                "case {label:?}: {merged} distinct keys is past the limit of {limit}, answered \
                 {tag_status} {tag_body:?}"
            );
            assert_eq!(
                stored.len(),
                case.seeded,
                "case {label:?}: a refused TagResource must store none of its tags, got {stored:?}"
            );
        }

        // The point of the row: one merged count, one answer — the status, the
        // error code and the message are the same whichever entry point asked.
        assert_eq!(
            open_status, tag_status,
            "case {merged:?} merged keys: the two entry points answered {open_status} \
             (OpenTunnel) and {tag_status} (TagResource)"
        );
        assert_eq!(
            refusal(&open_body),
            refusal(&tag_body),
            "case {merged:?} merged keys: the two entry points answered {open_body:?} \
             (OpenTunnel) and {tag_body:?} (TagResource)"
        );
        if !within {
            let message = open_body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&limit.to_string()) && message.contains(&merged.to_string()),
                "case {merged:?} merged keys: the refusal must name the limit {limit} and the \
                 count {merged} the request would reach, got {message:?}"
            );
        }
    }
}
