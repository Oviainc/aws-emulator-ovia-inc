// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, asserted over a class of keys rather than
//! over the one request that exposed it.
//!
//! RULE: a caller-supplied tag key in the namespace AWS reserves for the tags it
//! applies itself (`state::RESERVED_TAG_KEY_PREFIX`, matched however it is
//! cased) is refused with a 400 `ValidationException` wherever tags enter the
//! service — `OpenTunnel`'s `tags`, `TagResource`'s `tags`, `UntagResource`'s
//! `tagKeys` — and the refused request neither stores nor removes anything. A
//! key that merely resembles the prefix is an ordinary tag and round-trips.
//!
//! The prefix itself is read from the crate rather than re-spelled here, so the
//! rows and the handlers cannot drift apart.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::RESERVED_TAG_KEY_PREFIX;
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

/// A modelled `TagList` as a key -> value map.
fn tag_map(tags: Option<&Value>) -> BTreeMap<String, String> {
    tags.and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|tag| {
                    Some((
                        tag["key"].as_str()?.to_string(),
                        tag["value"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let (status, listed) = call(router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource({arn}) answered {status} {listed:?}"
    );
    tag_map(listed.get("tags"))
}

/// Open one tunnel carrying `tags` and return its `tunnelArn`.
async fn open_tunnel_with(router: &MultiServiceRouter, case: &str, tags: Value) -> String {
    let (status, opened) = call(
        router,
        "OpenTunnel",
        &json!({
            "description": case,
            "destinationConfig": {"thingName": "thing-reserved", "services": ["SSH"]},
            "tags": tags,
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: OpenTunnel answered {opened:?}");
    opened["tunnelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Keys inside the reserved namespace, differing along what the rule
/// generalises over: what follows the prefix (nothing, one character, a real AWS
/// system key, a non-ASCII word), the letter case of the prefix, and the length
/// up to `TagKey`'s modelled maximum of 128 characters
/// (`smithy_query(query="shape", shape="com.amazonaws.iotsecuretunneling#TagKey")`
/// -> `length(min: 1, max: 128)`, pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`).
fn reserved_keys() -> Vec<(&'static str, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    vec![
        ("the prefix with nothing after it", prefix.to_string()),
        (
            "a key AWS CloudFormation itself applies",
            format!("{prefix}cloudformation:stack-name"),
        ),
        ("a one-character suffix", format!("{prefix}x")),
        ("the prefix upper-cased", format!("{}owner", prefix.to_uppercase())),
        ("the prefix in mixed case", "Aws:Created-By".to_string()),
        (
            "a non-ASCII suffix, which the key pattern admits",
            format!("{prefix}Проект"),
        ),
        (
            "the longest key the model admits",
            format!(
                "{prefix}{}",
                "k".repeat(128 - prefix.chars().count())
            ),
        ),
    ]
}

/// Keys that only RESEMBLE the reserved prefix, so the rule must not touch
/// them: the prefix without its colon, the prefix as a mere substring, and the
/// prefix pushed off the front by a character the key pattern admits.
fn lookalike_keys() -> Vec<(&'static str, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    let bare = prefix.trim_end_matches(':').to_string();
    vec![
        ("the prefix without its colon", bare.clone()),
        ("a word that merely opens with the letters", format!("{bare}ome")),
        ("another colon-separated namespace", format!("{bare}s:billing")),
        ("the prefix later in the key", format!("team-{prefix}billing")),
        ("the prefix after a leading space", format!(" {prefix}billing")),
    ]
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_wherever_tags_enter_the_service() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // One live tunnel carrying an ordinary tag, so every refusal below is about
    // the key named and not about a missing resource or an empty tag store.
    let arn = open_tunnel_with(
        &router,
        "the live tunnel",
        json!([{"key": "env", "value": "test"}]),
    )
    .await;

    for (label, key) in reserved_keys() {
        // Each entry point, with the reserved key alone and beside an ordinary
        // one: the whole request is refused either way.
        let requests: Vec<(&str, Value)> = vec![
            (
                "OpenTunnel",
                json!({
                    "destinationConfig": {"thingName": "thing-reserved", "services": ["SSH"]},
                    "tags": [{"key": key, "value": "x"}],
                }),
            ),
            (
                "OpenTunnel",
                json!({
                    "destinationConfig": {"thingName": "thing-reserved", "services": ["SSH"]},
                    "tags": [{"key": "ordinary", "value": "x"}, {"key": key, "value": "x"}],
                }),
            ),
            (
                "TagResource",
                json!({"resourceArn": arn, "tags": [{"key": key, "value": "x"}]}),
            ),
            (
                "TagResource",
                json!({
                    "resourceArn": arn,
                    "tags": [{"key": "also-ordinary", "value": "x"}, {"key": key, "value": "x"}],
                }),
            ),
            (
                "UntagResource",
                json!({"resourceArn": arn, "tagKeys": [key]}),
            ),
            (
                "UntagResource",
                json!({"resourceArn": arn, "tagKeys": ["env", key]}),
            ),
        ];

        for (operation, input) in requests {
            let case = format!("{label} ({key:?}) on {operation}");
            let tunnels_before = engine.state::<TunnelState>().lock().tunnels.len();
            let (status, body) = call(&router, operation, &input).await;

            assert_eq!(
                status, 400,
                "case {case:?}: a reserved key must be refused, answered {status} {body:?}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("ValidationException"),
                "case {case:?}: answered {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(key.as_str()),
                "case {case:?}: the refusal must name the offending key, got {message:?}"
            );

            // Nothing the refused request carried took effect: no tunnel, no
            // new tag, and the ordinary tag it named is still there.
            let tunnels_after = engine.state::<TunnelState>().lock().tunnels.len();
            assert_eq!(
                tunnels_after, tunnels_before,
                "case {case:?}: a refused request must store no tunnel"
            );
            let stored = listed_tags(&router, &arn).await;
            assert_eq!(
                stored,
                BTreeMap::from([("env".to_string(), "test".to_string())]),
                "case {case:?}: a refused request must leave the tag set alone, got {stored:?}"
            );
        }
    }

    // The store never came to hold a key in the reserved namespace, by any route.
    let state = engine.state::<TunnelState>();
    let store = state.lock();
    let reserved: Vec<&String> = store
        .tags
        .values()
        .flat_map(BTreeMap::keys)
        .filter(|key| {
            key.get(..RESERVED_TAG_KEY_PREFIX.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
        })
        .collect();
    assert!(
        reserved.is_empty(),
        "no reserved key may ever reach the tag store, found {reserved:?}"
    );
}

#[tokio::test]
async fn keys_that_only_resemble_the_reserved_prefix_are_ordinary_tags() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for (label, key) in lookalike_keys() {
        // Set at open time, read back, overwritten through TagResource, read
        // back again, then removed through UntagResource: the whole tag
        // lifecycle a reserved key is denied.
        let case = format!("{label} ({key:?})");
        let arn = open_tunnel_with(&router, &case, json!([{"key": key, "value": "opened"}])).await;
        assert_eq!(
            listed_tags(&router, &arn).await,
            BTreeMap::from([(key.clone(), "opened".to_string())]),
            "case {case:?}: OpenTunnel must store an ordinary key"
        );

        let (status, tagged) = call(
            &router,
            "TagResource",
            &json!({"resourceArn": arn, "tags": [{"key": key, "value": "tagged"}]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must accept an ordinary key, answered {status} {tagged:?}"
        );
        assert_eq!(
            listed_tags(&router, &arn).await,
            BTreeMap::from([(key.clone(), "tagged".to_string())]),
            "case {case:?}: TagResource must overwrite the stored value"
        );

        // DescribeTunnel reads the same tag store, so the round trip holds there too.
        let tunnel_id = arn.split('/').next_back().unwrap_or_default().to_string();
        let (status, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTunnel answered {status} {described:?}"
        );
        assert_eq!(
            tag_map(described["tunnel"].get("tags")),
            BTreeMap::from([(key.clone(), "tagged".to_string())]),
            "case {case:?}: DescribeTunnel must report the same tags"
        );

        let (status, untagged) = call(
            &router,
            "UntagResource",
            &json!({"resourceArn": arn, "tagKeys": [key]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource must accept an ordinary key, answered {status} \
             {untagged:?}"
        );
        assert!(
            listed_tags(&router, &arn).await.is_empty(),
            "case {case:?}: UntagResource must remove an ordinary key"
        );
    }
}
