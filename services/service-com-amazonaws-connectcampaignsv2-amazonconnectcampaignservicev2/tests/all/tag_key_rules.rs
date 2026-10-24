// SPDX-License-Identifier: CC0-1.0

//! The tag rules every path that accepts tags is held to, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handler together. The rules under test:
//!
//! * a tag key a request supplies must satisfy the key shape's modelled `@length` and
//!   `@pattern`, whatever operation carries it, and a key that does not is HTTP 400
//!   `ValidationException` with nothing written;
//! * a resource holds at most [`MAX_TAGS_PER_RESOURCE`] tags on every path that can
//!   leave it holding tags — the create that is born with a tag map and the merge that
//!   adds to one — so no resource comes to hold a map a tag call could not have
//!   produced;
//! * a key outside the key shape is refused before the ARN is resolved, an input
//!   constraint preceding resource resolution;
//! * a refused call is inert: the campaign count, and the tag map of the campaign it
//!   named, are exactly what they were before it.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    MAX_TAGS_PER_RESOURCE, ONBOARDING_JOB_DURATION, TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH,
};
use smithy_server_runtime::MultiServiceRouter;

// One harness per crate: the routing header, the router under test and the
// request/response plumbing live in `tests/all/common.rs`.
use super::common::{call, error_code, router};

/// Onboard `instance` and wait out the span the clock owes its onboarding job, so the
/// instance is a campaign target by the time this returns.
async fn onboard(router: &MultiServiceRouter, region: &str, instance: &str) {
    let (status, body) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {instance:?}: expected onboarding to start, got {status} {body}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(100)).await;
}

/// The `{arn}` path label of `/v2/tags/{arn}`, percent-encoded as a single segment.
fn encoded(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// A JSON tag map from `(key, value)` pairs.
fn tag_map<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
    Value::Object(map)
}

/// `count` distinct legal tag keys, each with a value: the bulk a count-bound case
/// needs, in keys the key shape admits (ASCII letters and digits).
fn generated_tags(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("key{index:04}"), Value::String(format!("v{index}")));
    }
    Value::Object(map)
}

/// The tag map a `ListTagsForResource` / `CreateCampaign` / `DescribeCampaign`
/// response carries, as an ordered map so a mismatch prints deterministically.
fn tags_of(response: &Value) -> BTreeMap<String, String> {
    response
        .get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or("<not a string>").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every campaign the store holds, walked page by page through `ListCampaigns` — the
/// reading a refused create must leave untouched.
async fn campaign_count(router: &MultiServiceRouter, region: &str) -> usize {
    let mut total = 0;
    let mut next_token: Option<String> = None;
    loop {
        let request = match &next_token {
            Some(token) => json!({"maxResults": 50, "nextToken": token}),
            None => json!({"maxResults": 50}),
        };
        let (status, body) = call(router, region, "POST", "/v2/campaigns-summary", &request).await;
        assert_eq!(
            status, 200,
            "ListCampaigns {request}: expected 200, got {status} {body}"
        );
        total += body
            .get("campaignSummaryList")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        next_token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return total;
        }
    }
}

/// `ListTagsForResource` for `arn`, asserting it answered, and returning the stored
/// tag map.
async fn stored_tags(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/tags/{}", encoded(arn)),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {status} {body}"
    );
    tags_of(&body)
}

/// The tag keys the key shape admits and refuses.
///
/// Source: `TagKey`, the shape a tag map's key and a tag-key list's member both
/// target (`smithy_query query="shape" shape="TagKey"`): `@length` 1..128 and
/// `@pattern ^(?!aws:)[a-zA-Z+-=._:/]+$`, whose character class spells ASCII letters,
/// `_` and the `+`..`=` range (`+ , - . / 0-9 : ; < =`). The rows differ along the
/// character used, where in the key it sits, the key's length (both published bounds
/// and one past each) and whether the reserved prefix the pattern's leading lookahead
/// excludes is at the start of the key or inside it.
fn key_cases() -> Vec<(String, bool)> {
    let at_max_length = "a".repeat(TAG_KEY_MAX_LENGTH);
    let past_max_length = "a".repeat(TAG_KEY_MAX_LENGTH + 1);
    let at_min_length = "a".repeat(TAG_KEY_MIN_LENGTH);
    [
        // Admitted.
        ("Environment", true),
        ("cost-center", true),
        ("a_b.c:d/e", true),
        ("plus+and=equals", true),
        ("team0123456789", true),
        // The end of the `+`..`=` range, which spells punctuation a reader would not
        // guess at from the class's literal members.
        ("semi;colon<", true),
        // The lookahead is anchored at the start, so the reserved prefix elsewhere in
        // the key is an ordinary run of admitted characters.
        (":aws:nested", true),
        // The reserved prefix is `aws:`; `aws` on its own is not it.
        ("aws", true),
        (at_min_length.as_str(), true),
        (at_max_length.as_str(), true),
        // Refused.
        ("aws:reserved", false),
        ("aws:", false),
        ("", false),
        (past_max_length.as_str(), false),
        ("has space", false),
        ("bang!", false),
        ("percent%", false),
        ("tag@key", false),
        ("greater>than", false),
        ("héllo", false),
    ]
    .into_iter()
    .map(|(key, accepted)| (key.to_string(), accepted))
    .collect()
}

/// A tag key is held to the key shape's modelled bounds on every operation that takes
/// one: a key outside them is refused with HTTP 400 `ValidationException` and nothing
/// is written, and a key inside them round-trips through the readers.
#[tokio::test]
async fn a_tag_key_is_held_to_the_key_shape_on_create_and_on_tag() {
    let region = "eu-west-2";
    let router = router();
    onboard(&router, region, "instance-1").await;

    // The campaign the merge path is exercised against, with one tag already on it so
    // a refused merge has something to leave alone.
    let (status, host) = call(
        &router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": "merge target",
            "connectInstanceId": "instance-1",
            "tags": {"Keep": "me"},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"merge target\": expected 200, got {status} {host}"
    );
    let host_arn = host["arn"].as_str().expect("a minted arn").to_string();

    for (index, (key, accepted)) in key_cases().into_iter().enumerate() {
        let case = format!("key {key:?} ({} chars)", key.chars().count());
        // A campaign name identifies a campaign within its Connect instance, so each
        // row creates under a name of its own rather than colliding with the last.
        let name = format!("tagged {index}");
        let tags = tag_map([(key.as_str(), "value")]);
        let before = campaign_count(&router, region).await;

        // The create path.
        let (status, body) = call(
            &router,
            region,
            "PUT",
            "/v2/campaigns",
            &json!({"name": name, "connectInstanceId": "instance-1", "tags": tags}),
        )
        .await;
        let after = campaign_count(&router, region).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: CreateCampaign expected 200, got {status} {body}"
            );
            assert_eq!(
                tags_of(&body),
                BTreeMap::from([(key.clone(), "value".to_string())]),
                "case {case:?}: CreateCampaign echoed {:?}",
                tags_of(&body)
            );
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: expected the create to store one campaign, count went \
                 {before} -> {after}"
            );
            // The stored record is what the reader serves, not just what the create
            // echoed.
            let arn = body["arn"].as_str().expect("a minted arn");
            assert_eq!(
                stored_tags(&router, region, arn, &case).await,
                BTreeMap::from([(key.clone(), "value".to_string())]),
                "case {case:?}: ListTagsForResource disagreed with the create"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: CreateCampaign expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {} {body}",
                error_code(&body)
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused create stored a campaign, count went \
                 {before} -> {after}"
            );
        }

        // The merge path, on the same key.
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/tags/{}", encoded(&host_arn)),
            &json!({"tags": tags}),
        )
        .await;
        let merged = stored_tags(&router, region, &host_arn, &case).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: TagResource expected 200, got {status} {body}"
            );
            assert_eq!(
                merged.get(&key).map(String::as_str),
                Some("value"),
                "case {case:?}: expected the merged map to carry the key, got {merged:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: TagResource expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {} {body}",
                error_code(&body)
            );
            assert!(
                !merged.contains_key(&key),
                "case {case:?}: a refused merge wrote the key, stored map is {merged:?}"
            );
            assert_eq!(
                merged.get("Keep").map(String::as_str),
                Some("me"),
                "case {case:?}: a refused merge disturbed the existing tags: {merged:?}"
            );
        }
    }
}

/// A tag key outside the key shape is refused BEFORE the ARN is resolved: an input
/// constraint precedes resource resolution, so a malformed key answers
/// `ValidationException` even where the ARN names nothing, while the same ARN with a
/// legal key still answers its own resolution error.
///
/// The rows differ along how the ARN fails to resolve (a resource path this service
/// mints nothing under, versus a campaign ARN whose id the store does not hold) and
/// along whether the key is one the shape admits.
#[tokio::test]
async fn a_malformed_tag_key_is_refused_before_the_arn_is_resolved() {
    let region = "eu-west-2";
    let router = router();

    let unresolvable = "arn:aws:connect:us-east-1:000000000000:instance/000000000000";
    let unknown_campaign = "arn:aws:connect-campaigns:us-east-1:000000000000:campaign/999999999999";

    // (case, arn, key, expected status, expected error code)
    let cases: &[(&str, &str, &str, u16, &str)] = &[
        (
            "another service's ARN, legal key",
            unresolvable,
            "Environment",
            400,
            "BadRequestException",
        ),
        (
            "another service's ARN, reserved key",
            unresolvable,
            "aws:reserved",
            400,
            "ValidationException",
        ),
        (
            "an id the store does not hold, legal key",
            unknown_campaign,
            "Environment",
            404,
            "NotFoundException",
        ),
        (
            "an id the store does not hold, reserved key",
            unknown_campaign,
            "aws:reserved",
            400,
            "ValidationException",
        ),
        (
            "an id the store does not hold, over-long key",
            unknown_campaign,
            &"a".repeat(TAG_KEY_MAX_LENGTH + 1),
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, key, expected_status, expected_code) in cases {
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/tags/{}", encoded(arn)),
            &json!({"tags": tag_map([(*key, "value")])}),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with(expected_code),
            "case {case:?}: expected {expected_code}, got {} {body}",
            error_code(&body)
        );
    }
}

/// A campaign cannot be born past the per-resource tag ceiling, and cannot be merged
/// past it either: both paths refuse at the same count, with HTTP 400
/// `ValidationException`, and store nothing.
///
/// The rows differ along the tag count: none, one, just under the ceiling, exactly at
/// it, one past it, and well past it.
#[tokio::test]
async fn neither_a_create_nor_a_merge_leaves_a_resource_past_the_tag_ceiling() {
    let region = "eu-west-2";
    let router = router();
    onboard(&router, region, "instance-1").await;

    for count in [
        0,
        1,
        MAX_TAGS_PER_RESOURCE - 1,
        MAX_TAGS_PER_RESOURCE,
        MAX_TAGS_PER_RESOURCE + 1,
        MAX_TAGS_PER_RESOURCE + 10,
    ] {
        let case = format!("{count} tags at create");
        let tags = generated_tags(count);
        let before = campaign_count(&router, region).await;
        let (status, body) = call(
            &router,
            region,
            "PUT",
            "/v2/campaigns",
            &json!({
                "name": format!("bulk tagged {count}"),
                "connectInstanceId": "instance-1",
                "tags": tags,
            }),
        )
        .await;
        let after = campaign_count(&router, region).await;

        if count <= MAX_TAGS_PER_RESOURCE {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body}"
            );
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: expected one campaign stored, count went {before} -> {after}"
            );
            let arn = body["arn"].as_str().expect("a minted arn");
            let stored = stored_tags(&router, region, arn, &case).await;
            assert_eq!(
                stored.len(),
                count,
                "case {case:?}: expected {count} stored tags, got {}",
                stored.len()
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {} {body}",
                error_code(&body)
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused create stored a campaign, count went \
                 {before} -> {after}"
            );
        }
    }

    // The merge path meets the same ceiling: a campaign already at it takes a new
    // value for a key it holds, and refuses one more key.
    let (status, full) = call(
        &router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": "at the ceiling",
            "connectInstanceId": "instance-1",
            "tags": generated_tags(MAX_TAGS_PER_RESOURCE),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create at the ceiling\": expected 200, got {status} {full}"
    );
    let arn = full["arn"].as_str().expect("a minted arn").to_string();

    // (case, the tags the merge carries, expected status)
    let overwrite = tag_map([("key0000", "fresh")]);
    let one_more = tag_map([("oneMore", "too many")]);
    let several_more = generated_tags(MAX_TAGS_PER_RESOURCE + 5);
    for (case, tags, expected) in [
        ("re-tag a key already held", &overwrite, 200),
        ("one key past the ceiling", &one_more, 400),
        ("several keys past the ceiling", &several_more, 400),
    ] {
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/tags/{}", encoded(&arn)),
            &json!({"tags": tags}),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {} {body}",
                error_code(&body)
            );
        }
        let stored = stored_tags(&router, region, &arn, case).await;
        assert_eq!(
            stored.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: expected the campaign to still hold {MAX_TAGS_PER_RESOURCE} tags, \
             got {}",
            stored.len()
        );
    }

    // The accepted merge above replaced a value rather than adding a key, and
    // `DescribeCampaign` reads the same map `ListTagsForResource` does.
    let (status, described) = call(
        &router,
        region,
        "GET",
        &format!("/v2/campaigns/{}", full["id"].as_str().expect("a minted id")),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"describe at the ceiling\": expected 200, got {status} {described}"
    );
    let described_tags = described
        .pointer("/campaign/tags")
        .cloned()
        .map(|tags| tags_of(&json!({"tags": tags})))
        .unwrap_or_default();
    assert_eq!(
        described_tags.get("key0000").map(String::as_str),
        Some("fresh"),
        "case \"describe at the ceiling\": expected the merged value, got {described_tags:?}"
    );
}
