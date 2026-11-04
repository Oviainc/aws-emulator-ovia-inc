// SPDX-License-Identifier: CC0-1.0

//! The per-tunnel tag-key count, asserted at BOTH entry points that apply it.
//!
//! RULE: the number of DISTINCT tag keys a tunnel would end up carrying, after
//! merging the keys a request names into the ones already stored, may not exceed
//! `state::MAX_TAGS_PER_TUNNEL`; a request past it is refused with the modelled
//! `LimitExceededException` and stores nothing. Both writers of the
//! tag store — `OpenTunnel`'s `tags` and `TagResource`'s `tags` — count through
//! the same helper on the state, so one table of rows is asserted against both
//! and the two paths cannot answer differently.
//!
//! Rows differ along what the count generalises over: the number of entries, the
//! number of distinct keys among them, how the duplicates are arranged, the
//! order the keys arrive in, and each side of the published bound. The bound
//! itself is read from the crate rather than re-spelled here.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::MAX_TAGS_PER_TUNNEL;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
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

/// A modelled `TagList` naming `keys` in the order given, duplicates included.
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

/// One request's tag list: a label and the keys it names, duplicates included.
/// `TagList` is modelled `length(min: 1, max: 200)`
/// (`smithy_query(query="shape", shape="com.amazonaws.iotsecuretunneling#TagList")`),
/// so every row stays within 200 entries and the bound under test is the
/// distinct-key count, not the list length.
fn cases() -> Vec<(String, Vec<String>)> {
    let limit = MAX_TAGS_PER_TUNNEL;
    let mut at_the_limit_with_a_repeat = distinct_keys(limit, "k");
    at_the_limit_with_a_repeat.push("k0".to_string());

    let mut past_the_limit_doubled = distinct_keys(limit + 1, "k");
    past_the_limit_doubled.extend(distinct_keys(limit + 1, "k"));

    let interleaved: Vec<String> = (0..10)
        .map(|index| if index % 2 == 0 { "left" } else { "right" })
        .map(str::to_string)
        .collect();

    let mut descending = distinct_keys(10, "k");
    descending.reverse();

    vec![
        ("one key".to_string(), vec!["only".to_string()]),
        (
            format!("{limit} keys, every one distinct: the limit itself"),
            distinct_keys(limit, "k"),
        ),
        (
            format!("{} distinct keys: one past the limit", limit + 1),
            distinct_keys(limit + 1, "k"),
        ),
        (
            format!("{} distinct keys: two past the limit", limit + 2),
            distinct_keys(limit + 2, "k"),
        ),
        (
            format!("{} entries but {limit} distinct keys", limit + 1),
            at_the_limit_with_a_repeat,
        ),
        (
            format!("{} entries naming one key over and over", limit + 10),
            vec!["repeated".to_string(); limit + 10],
        ),
        (
            "two keys interleaved across ten entries".to_string(),
            interleaved,
        ),
        (
            "ten distinct keys in descending order".to_string(),
            descending,
        ),
        (
            format!(
                "{} entries: one past the limit, every key named twice",
                2 * (limit + 1)
            ),
            past_the_limit_doubled,
        ),
    ]
}

/// Open a tunnel with no tags and return its `tunnelArn`, so a `TagResource`
/// row starts from an empty tag set exactly as an `OpenTunnel` row does.
async fn open_untagged(router: &MultiServiceRouter, case: &str) -> String {
    let (status, opened) = call(
        router,
        "OpenTunnel",
        &json!({
            "description": case,
            "destinationConfig": {"thingName": "thing-tag-count", "services": ["SSH"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: OpenTunnel answered {status} {opened:?}"
    );
    opened["tunnelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn both_tagging_entry_points_count_the_same_distinct_keys() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for (label, keys) in cases() {
        let distinct: BTreeSet<String> = keys.iter().cloned().collect();
        let accepted = distinct.len() <= MAX_TAGS_PER_TUNNEL;

        // Entry point 1: tags supplied at open time.
        let case = format!("{label} via OpenTunnel");
        let before = engine.state::<TunnelState>().lock().tunnels.len();
        let (status, body) = call(
            &router,
            "OpenTunnel",
            &json!({
                "destinationConfig": {"thingName": "thing-tag-count", "services": ["SSH"]},
                "tags": tag_list(&keys),
            }),
        )
        .await;
        let after = engine.state::<TunnelState>().lock().tunnels.len();
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {} distinct keys is within the limit of {MAX_TAGS_PER_TUNNEL}, \
                 answered {status} {body:?}",
                distinct.len()
            );
            let arn = body["tunnelArn"].as_str().unwrap_or_default().to_string();
            let stored = stored_keys(&router, &arn).await;
            assert_eq!(
                stored, distinct,
                "case {case:?}: the stored keys must be exactly the distinct keys named, got \
                 {stored:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {} distinct keys is past the limit of {MAX_TAGS_PER_TUNNEL}, \
                 answered {status} {body:?}",
                distinct.len()
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("LimitExceededException"),
                "case {case:?}: answered {body:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused open must store no tunnel, went from {before} to {after}"
            );
        }

        // Entry point 2: the same keys through `TagResource`, on a tunnel whose
        // tag set starts empty — so the only difference between the two rows is
        // which operation carried the keys.
        let case = format!("{label} via TagResource");
        let arn = open_untagged(&router, &case).await;
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"resourceArn": arn, "tags": tag_list(&keys)}),
        )
        .await;
        let stored = stored_keys(&router, &arn).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {} distinct keys is within the limit of {MAX_TAGS_PER_TUNNEL}, \
                 answered {status} {body:?}",
                distinct.len()
            );
            assert_eq!(
                stored, distinct,
                "case {case:?}: the stored keys must be exactly the distinct keys named, got \
                 {stored:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {} distinct keys is past the limit of {MAX_TAGS_PER_TUNNEL}, \
                 answered {status} {body:?}",
                distinct.len()
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("LimitExceededException"),
                "case {case:?}: answered {body:?}"
            );
            assert!(
                stored.is_empty(),
                "case {case:?}: a refused TagResource must store none of its tags, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_count_is_of_the_keys_the_tunnel_would_end_up_with() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // A tunnel opened at the limit: the keys it already holds are what a later
    // request is counted against.
    let seeded = distinct_keys(MAX_TAGS_PER_TUNNEL, "seed");
    let (status, opened) = call(
        &router,
        "OpenTunnel",
        &json!({
            "destinationConfig": {"thingName": "thing-tag-count", "services": ["SSH"]},
            "tags": tag_list(&seeded),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a tunnel at the limit must open, answered {status} {opened:?}"
    );
    let arn = opened["tunnelArn"].as_str().unwrap_or_default().to_string();
    let seeded_keys: BTreeSet<String> = seeded.iter().cloned().collect();

    // Rows: the keys a follow-up `TagResource` names, and whether the resulting
    // key set stays within the limit. Differ along whether the keys are already
    // held, how many are new, and how often each is repeated.
    let follow_ups: Vec<(String, Vec<String>, bool)> = vec![
        (
            "every key already held, once each".to_string(),
            seeded.clone(),
            true,
        ),
        (
            "one held key, named five times".to_string(),
            vec!["seed0".to_string(); 5],
            true,
        ),
        (
            "one new key".to_string(),
            vec!["fresh".to_string()],
            false,
        ),
        (
            "one new key named twenty times".to_string(),
            vec!["fresh".to_string(); 20],
            false,
        ),
        (
            "held keys beside one new key".to_string(),
            {
                let mut keys = seeded.clone();
                keys.push("fresh".to_string());
                keys
            },
            false,
        ),
    ];

    for (label, keys, accepted) in follow_ups {
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"resourceArn": arn, "tags": tag_list(&keys)}),
        )
        .await;
        let stored = stored_keys(&router, &arn).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: the key set does not grow past the limit, answered {status} \
                 {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: the key set would pass the limit of {MAX_TAGS_PER_TUNNEL}, \
                 answered {status} {body:?}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("LimitExceededException"),
                "case {label:?}: answered {body:?}"
            );
        }
        assert_eq!(
            stored, seeded_keys,
            "case {label:?}: the stored key set must be unchanged, got {stored:?}"
        );
    }

    // Freeing one key makes room for exactly one new key, at either entry point's
    // shared count.
    let (status, untagged) = call(
        &router,
        "UntagResource",
        &json!({"resourceArn": arn, "tagKeys": ["seed0"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "UntagResource answered {status} {untagged:?}"
    );
    let (status, tagged) = call(
        &router,
        "TagResource",
        &json!({"resourceArn": arn, "tags": tag_list(&["fresh".to_string()])}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a freed slot\": a new key must be accepted, answered {status} {tagged:?}"
    );
    let stored = stored_keys(&router, &arn).await;
    assert_eq!(
        stored.len(),
        MAX_TAGS_PER_TUNNEL,
        "case \"a freed slot\": the tunnel must be back at the limit, got {stored:?}"
    );
    assert!(
        stored.contains("fresh") && !stored.contains("seed0"),
        "case \"a freed slot\": the new key must have replaced the freed one, got {stored:?}"
    );
}
