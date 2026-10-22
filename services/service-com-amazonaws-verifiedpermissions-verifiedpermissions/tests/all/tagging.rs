// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the resource tagging loop:
//! `TagResource` -> `ListTagsForResource` -> `UntagResource`.
//!
//! The rule under test: a tagging call MERGES the request's key-value pairs into
//! the tag set the named resource already holds — a new key is appended, a key
//! already present takes the new value — and the reader reports exactly that
//! persisted set; the merge is refused with `TooManyTagsException` when it would
//! leave the resource over the per-resource tag limit, and an ARN no live
//! resource carries is `ResourceNotFoundException` on every one of the three
//! operations.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! emulator binary serves, over ONE shared engine, reusing the harness in
//! `harness.rs` — so a tag is only visible to a later request if it really
//! reached state.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, error_code, ok, router};

/// `MAX_TAGS_PER_RESOURCE` in `src/state.rs`, which the model states twice: in
/// `TagResource`'s documentation ("You can associate as many as 50 tags with a
/// resource") and in `TooManyTagsException`'s ("the limit (50)").
///
/// Written out here rather than imported on purpose: this suite pins the
/// PUBLISHED quota, so a change to the implementation constant has to fail this
/// test instead of moving with it.
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One tag past the per-resource limit: the smallest request that must be
/// refused on an untagged resource.
const OVER_THE_LIMIT: usize = MAX_TAGS_PER_RESOURCE + 1;

/// `TagKey`'s modelled length bound, from
/// `smithy_query(query="shape", shape="TagKey")`: 1..128.
const MAX_TAG_KEY_LEN: usize = 128;

/// `TagValue`'s modelled length bound, from
/// `smithy_query(query="shape", shape="TagValue")`: 0..256.
const MAX_TAG_VALUE_LEN: usize = 256;

/// Create a policy store, optionally with create-time tags, and return its ARN.
async fn store_arn(router: &MultiServiceRouter, tags: Option<Value>) -> String {
    let mut request = json!({ "validationSettings": { "mode": "OFF" } });
    if let Some(tags) = tags {
        request["tags"] = tags;
    }
    let created = ok(router, "CreatePolicyStore", &request).await;
    created["arn"]
        .as_str()
        .expect("arn is required on the CreatePolicyStore response")
        .to_string()
}

/// The tag set `ListTagsForResource` reports for `arn`, as an ordered map so two
/// sets compare regardless of wire order.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let body = ok(
        router,
        "ListTagsForResource",
        &json!({ "resourceArn": arn }),
    )
    .await;
    as_map(&body["tags"])
}

/// A JSON object as an ordered `String -> String` map.
fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// A tag map of `count` distinct keys, prefixed with `prefix`.
fn tag_map(prefix: &str, count: usize) -> Value {
    let map: serde_json::Map<String, Value> = (0..count)
        .map(|index| (format!("{prefix}{index:03}"), json!("v")))
        .collect();
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// The merge rule
// ---------------------------------------------------------------------------

/// Successive `TagResource` calls merge into the resource's persisted tag set,
/// and `ListTagsForResource` reports exactly that set.
#[tokio::test]
async fn tag_resource_merges_into_the_persisted_tag_set() {
    /// One row: the tags one `TagResource` call carries, and the whole set the
    /// resource must hold afterwards.
    struct Step {
        case: &'static str,
        tags: Value,
        expected: Value,
    }

    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);

    // Rows differ along what the merge has to do: append one key, append
    // several at once, replace an existing key's value, write an empty value,
    // write a key and a value at the modelled length bounds, write a
    // non-ASCII key, and repeat a call that changes nothing.
    let steps = vec![
        Step {
            case: "first key appended",
            tags: json!({ "env": "test" }),
            expected: json!({ "team": "photos", "env": "test" }),
        },
        Step {
            case: "several keys appended at once",
            tags: json!({ "owner": "alice", "tier": "gold" }),
            expected: json!({ "team": "photos", "env": "test", "owner": "alice", "tier": "gold" }),
        },
        Step {
            case: "existing key replaced",
            tags: json!({ "env": "prod" }),
            expected: json!({ "team": "photos", "env": "prod", "owner": "alice", "tier": "gold" }),
        },
        Step {
            case: "create-time key replaced",
            tags: json!({ "team": "videos" }),
            expected: json!({ "team": "videos", "env": "prod", "owner": "alice", "tier": "gold" }),
        },
        Step {
            case: "empty value is a value",
            tags: json!({ "owner": "" }),
            expected: json!({ "team": "videos", "env": "prod", "owner": "", "tier": "gold" }),
        },
        Step {
            case: "key and value at the modelled length bounds",
            tags: json!({ long_key.clone(): long_value.clone() }),
            expected: json!({
                "team": "videos", "env": "prod", "owner": "", "tier": "gold",
                long_key.clone(): long_value.clone(),
            }),
        },
        Step {
            case: "non-ascii key",
            tags: json!({ "équipe": "photos/vidéos" }),
            expected: json!({
                "team": "videos", "env": "prod", "owner": "", "tier": "gold",
                long_key.clone(): long_value.clone(), "équipe": "photos/vidéos",
            }),
        },
        Step {
            case: "repeated call changes nothing",
            tags: json!({ "env": "prod" }),
            expected: json!({
                "team": "videos", "env": "prod", "owner": "", "tier": "gold",
                long_key.clone(): long_value.clone(), "équipe": "photos/vidéos",
            }),
        },
    ];

    let router = router();
    let arn = store_arn(&router, Some(json!({ "team": "photos" }))).await;

    // The create-time tags are what the resource starts from.
    assert_eq!(
        listed_tags(&router, &arn).await,
        as_map(&json!({ "team": "photos" })),
        "case \"create-time tags\": expected the tags CreatePolicyStore persisted, got {:?}",
        listed_tags(&router, &arn).await
    );

    for Step {
        case,
        tags,
        expected,
    } in &steps
    {
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from TagResource, got {status} {body}"
        );

        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed,
            as_map(expected),
            "case {case:?}: expected the merged tag set {:?}, got {listed:?}",
            as_map(expected)
        );

        // The same persisted set is what `GetPolicyStore` reports when asked for
        // tags: one tag set per resource, not two copies.
        let store_id = arn
            .rsplit_once('/')
            .map(|(_, id)| id)
            .expect("a policy store ARN ends in /<policyStoreId>");
        let fetched = ok(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": store_id, "tags": true }),
        )
        .await;
        assert_eq!(
            as_map(&fetched["tags"]),
            listed,
            "case {case:?}: GetPolicyStore reported a different tag set, got {fetched}"
        );
    }
}

/// `UntagResource` removes exactly the named keys, tolerates a key the resource
/// does not carry, and leaves everything else persisted.
#[tokio::test]
async fn untag_resource_removes_only_the_named_keys() {
    /// One row: the keys one `UntagResource` call names, and the set left after.
    struct Step {
        case: &'static str,
        keys: Value,
        expected: Value,
    }

    // Rows differ along how many keys are removed and whether they are present.
    let steps = &[
        Step {
            case: "one present key",
            keys: json!(["env"]),
            expected: json!({ "team": "photos", "owner": "alice", "tier": "gold" }),
        },
        Step {
            case: "an absent key is a no-op",
            keys: json!(["nosuchkey"]),
            expected: json!({ "team": "photos", "owner": "alice", "tier": "gold" }),
        },
        Step {
            case: "several keys at once, one of them absent",
            keys: json!(["owner", "tier", "nosuchkey"]),
            expected: json!({ "team": "photos" }),
        },
        Step {
            case: "the last key leaves an empty set",
            keys: json!(["team"]),
            expected: json!({}),
        },
        Step {
            case: "removing from an empty set",
            keys: json!(["team"]),
            expected: json!({}),
        },
    ];

    let router = router();
    let arn = store_arn(&router, None).await;
    ok(
        &router,
        "TagResource",
        &json!({
            "resourceArn": arn,
            "tags": { "team": "photos", "env": "test", "owner": "alice", "tier": "gold" },
        }),
    )
    .await;

    for Step {
        case,
        keys,
        expected,
    } in steps
    {
        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({ "resourceArn": arn, "tagKeys": keys }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UntagResource, got {status} {body}"
        );
        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed,
            as_map(expected),
            "case {case:?}: expected the remaining tag set {:?}, got {listed:?}",
            as_map(expected)
        );
    }
}

// ---------------------------------------------------------------------------
// The per-resource limit
// ---------------------------------------------------------------------------

/// The limit is on what the resource ENDS UP holding: a merge that would take it
/// past the limit is refused with `TooManyTagsException` and persists nothing,
/// while a merge that stays at or under it succeeds — even on a full resource,
/// when every key it carries is one the resource already has.
#[tokio::test]
async fn the_per_resource_tag_limit_refuses_the_merge_and_persists_nothing() {
    let router = router();

    // One request on an empty resource, one tag over the limit: refused.
    let fresh = store_arn(&router, None).await;
    let over = tag_map("over", OVER_THE_LIMIT);
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": fresh, "tags": over }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one request over the limit\": expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyTagsException",
        "case \"one request over the limit\": unexpected error code, got {body}"
    );
    assert_eq!(
        listed_tags(&router, &fresh).await,
        BTreeMap::new(),
        "case \"one request over the limit\": a refused merge must persist nothing, got {:?}",
        listed_tags(&router, &fresh).await
    );

    // Exactly at the limit, in one request: accepted.
    let full = store_arn(&router, None).await;
    let at_limit = tag_map("key", MAX_TAGS_PER_RESOURCE);
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": full, "tags": at_limit }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"exactly at the limit\": expected HTTP 200, got {status} {body}"
    );
    let at_limit_map = as_map(&at_limit);
    let listed = listed_tags(&router, &full).await;
    assert_eq!(
        listed, at_limit_map,
        "case \"exactly at the limit\": expected all {MAX_TAGS_PER_RESOURCE} tags persisted, got {listed:?}"
    );

    // One more KEY on a full resource: refused, and the full set is untouched.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": full, "tags": { "onemore": "v" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one key past a full resource\": expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyTagsException",
        "case \"one key past a full resource\": unexpected error code, got {body}"
    );
    assert_eq!(
        listed_tags(&router, &full).await,
        at_limit_map,
        "case \"one key past a full resource\": the stored set must be unchanged, got {:?}",
        listed_tags(&router, &full).await
    );

    // A replacement on a full resource does not grow it, so it is accepted.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": full, "tags": { "key000": "replaced" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"replacement on a full resource\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        listed_tags(&router, &full).await.get("key000").cloned(),
        Some("replaced".to_string()),
        "case \"replacement on a full resource\": expected the new value, got {:?}",
        listed_tags(&router, &full).await.get("key000")
    );

    // And the documented way back under the limit: remove a key, then the next
    // new key fits.
    ok(
        &router,
        "UntagResource",
        &json!({ "resourceArn": full, "tagKeys": ["key000"] }),
    )
    .await;
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": full, "tags": { "onemore": "v" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a freed slot accepts a new key\": expected HTTP 200, got {status} {body}"
    );
    let final_tags = listed_tags(&router, &full).await;
    assert_eq!(
        final_tags.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"a freed slot accepts a new key\": expected {MAX_TAGS_PER_RESOURCE} tags, got {final_tags:?}"
    );
    assert_eq!(
        final_tags.get("onemore").cloned(),
        Some("v".to_string()),
        "case \"a freed slot accepts a new key\": the new key must be persisted, got {final_tags:?}"
    );
}

// ---------------------------------------------------------------------------
// An ARN no resource carries
// ---------------------------------------------------------------------------

/// Every tagging operation answers `ResourceNotFoundException` (404) for an ARN
/// no live resource carries — whatever the ARN looks like, including one whose
/// store has been deleted.
#[tokio::test]
async fn tagging_an_arn_no_live_resource_carries_is_not_found() {
    let router = router();

    // A store created and then deleted: its ARN is well-formed and was real.
    let deleted = store_arn(&router, Some(json!({ "team": "photos" }))).await;
    let deleted_id = deleted
        .rsplit_once('/')
        .map(|(_, id)| id.to_string())
        .expect("a policy store ARN ends in /<policyStoreId>");
    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": deleted_id }),
    )
    .await;

    // Rows differ along the FORM of the ARN that names nothing.
    let arns: &[(&str, String)] = &[
        (
            "well-formed, never existed",
            "arn:aws:verifiedpermissions::000000000000:policy-store/PSneverexisted".to_string(),
        ),
        ("the ARN of a deleted store", deleted.clone()),
        (
            "another account's store",
            "arn:aws:verifiedpermissions::111122223333:policy-store/PSEXAMPLEabcdefg111111"
                .to_string(),
        ),
        (
            "another service's resource",
            "arn:aws:sqs:us-east-1:000000000000:some-queue".to_string(),
        ),
        ("a bare policy store id", "PSEXAMPLEabcdefg111111".to_string()),
    ];

    // Each operation carries its own required members beside `resourceArn`, so
    // a row is the operation plus how its request body is built.
    type RequestFor = fn(&str) -> Value;
    let operations: &[(&str, RequestFor)] = &[
        ("TagResource", |arn| {
            json!({ "resourceArn": arn, "tags": { "env": "test" } })
        }),
        ("ListTagsForResource", |arn| json!({ "resourceArn": arn })),
        ("UntagResource", |arn| {
            json!({ "resourceArn": arn, "tagKeys": ["env"] })
        }),
    ];

    for (arn_case, arn) in arns {
        for (operation, body_of) in operations {
            let (status, body) = call(&router, operation, &body_of(arn)).await;
            assert_eq!(
                status, 404,
                "case {operation:?}/{arn_case:?}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {operation:?}/{arn_case:?}: unexpected error code, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str().unwrap_or_default(),
                "POLICY_STORE",
                "case {operation:?}/{arn_case:?}: the only taggable resource is a policy store, got {body}"
            );
        }
    }
}

/// A resource that was never tagged reports an EMPTY tag map, not a 501 and not
/// an absent member.
#[tokio::test]
async fn an_untagged_resource_reports_an_empty_tag_map() {
    let router = router();
    let arn = store_arn(&router, None).await;
    let body = ok(
        &router,
        "ListTagsForResource",
        &json!({ "resourceArn": arn }),
    )
    .await;
    assert!(
        body["tags"].is_object(),
        "case \"untagged resource\": expected an empty tags object, got {body}"
    );
    assert_eq!(
        as_map(&body["tags"]),
        BTreeMap::new(),
        "case \"untagged resource\": expected no tags, got {body}"
    );
}
