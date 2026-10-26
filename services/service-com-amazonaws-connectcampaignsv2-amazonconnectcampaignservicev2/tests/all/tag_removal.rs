// SPDX-License-Identifier: CC0-1.0

//! Removing tags from a resource, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handler together. The rules under test:
//!
//! * removal is BY KEY: the resource keeps exactly the tags whose keys the request did
//!   not name, with the values it held, and loses the ones it did name — whatever those
//!   values were and whether or not the resource held them;
//! * removal is IDEMPOTENT (the operation is `@idempotent`), so naming a key twice, or
//!   repeating the whole call, leaves the same map;
//! * the one map removal writes is the one every reader projects, so
//!   `ListTagsForResource` and `DescribeCampaign` agree after it;
//! * a key outside the key shape's modelled bounds is refused HTTP 400
//!   `ValidationException` before the ARN is resolved, and an ARN that resolves to no
//!   campaign of the region the request was signed for is refused with the code the
//!   service answers for it — with nothing removed either way.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ONBOARDING_JOB_DURATION, TAG_KEY_MAX_LENGTH,
};
use smithy_server_runtime::MultiServiceRouter;

// One harness per crate: the routing header, the router under test and the
// request/response plumbing live in `tests/all/common.rs`.
use super::common::{call, error_code, router};

/// The most keys one `UntagResource` call may carry.
///
/// Source: `UntagResource$tagKeys`, whose target `TagKeyList` has `@length` 0..50
/// (`smithy_query query="operation" operation="UntagResource"`).
const MAX_TAG_KEYS_PER_CALL: usize = 50;

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

/// `text` as one percent-encoded query value: every character outside the unreserved
/// set of RFC 3986 (`A-Z a-z 0-9 - _ . ~`) is escaped.
///
/// A key is a value the CALLER chose, and a query string decodes `+` as a space while
/// using `&` and `=` as its own punctuation — yet all of those are characters the tag-key
/// shape admits, so none of them may reach the query string unescaped. The ARN path
/// label beside it is escaped by [`encoded`], this crate's existing test convention.
fn query_encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte))
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// `DELETE /v2/tags/{arn}` with one `tagKeys` query parameter per key — the binding
/// the model gives that member (`httpQuery: "tagKeys"` on a list).
fn untag_path(arn: &str, keys: &[String]) -> String {
    let mut path = format!("/v2/tags/{}", encoded(arn));
    for (index, key) in keys.iter().enumerate() {
        path.push(if index == 0 { '?' } else { '&' });
        path.push_str("tagKeys=");
        path.push_str(&query_encoded(key));
    }
    path
}

/// A JSON tag map from `(key, value)` pairs.
fn tag_map<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
    Value::Object(map)
}

/// The tag map a `ListTagsForResource` / `CreateCampaign` response carries, as an
/// ordered map so a mismatch prints deterministically.
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

/// The tag map `DescribeCampaign` projects for `id` — the same record
/// `ListTagsForResource` reads, reached through the other reader.
async fn described_tags(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/campaigns/{id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCampaign expected 200, got {status} {body}"
    );
    body.pointer("/campaign/tags")
        .cloned()
        .map(|tags| tags_of(&json!({"tags": tags})))
        .unwrap_or_default()
}

/// The tag map every row of the removal table starts from.
///
/// The keys span the forms `TagKey` admits — the shape `UntagResource$tagKeys`'s member
/// and a tag map's key both target (`smithy_query query="shape" shape="TagKey"`:
/// `@length` 1..128, `@pattern ^(?!aws:)[a-zA-Z+-=._:/]+$`) — so a row that removes one
/// of them exercises a key form rather than only a key.
fn starting_tags() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Environment", "prod"),
        ("cost-center", "42"),
        ("a_b.c:d/e", "nested"),
        ("plus+and=equals", "punctuation"),
        ("team0123456789", "squad"),
    ]
}

/// `count` distinct legal tag keys the starting map does not hold, for a row that fills
/// the per-call key bound.
fn absent_keys(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("absent{index:04}")).collect()
}

/// Create a campaign under `name` holding [`starting_tags`], and return its id and ARN.
async fn tagged_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
) -> (String, String) {
    let (status, body) = call(
        router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": name,
            "connectInstanceId": instance,
            "tags": tag_map(starting_tags()),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCampaign expected 200, got {status} {body}"
    );
    (
        body["id"].as_str().expect("a minted id").to_string(),
        body["arn"].as_str().expect("a minted arn").to_string(),
    )
}

/// A removal leaves the resource holding exactly the tags whose keys the request did
/// not name, with the values they had — and leaves the same map when the identical call
/// is made again.
///
/// The rows differ along the number of keys named (one, two, every one held, and the
/// per-call maximum), whether the keys are held by the resource at all (held, absent,
/// a mix), whether a key is named more than once, the form of the key (letters, digits,
/// `_`, and the punctuation the key shape's character class admits) and the order the
/// keys arrive in.
#[tokio::test]
async fn untag_removes_exactly_the_keys_the_request_names() {
    let region = "eu-west-2";
    let instance = "instance-1";
    let router = router();
    onboard(&router, region, instance).await;

    let held: Vec<String> = starting_tags()
        .into_iter()
        .map(|(key, _)| key.to_string())
        .collect();
    let mut every_key_reversed = held.clone();
    every_key_reversed.reverse();
    let mut full_call = absent_keys(MAX_TAG_KEYS_PER_CALL - 1);
    full_call.push("cost-center".to_string());

    // (case, the keys the call names)
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one key the resource holds", vec![held[0].clone()]),
        (
            "two keys the resource holds",
            vec![held[0].clone(), held[1].clone()],
        ),
        (
            "a key the resource does not hold",
            vec!["absent".to_string()],
        ),
        (
            "a held key and an absent one",
            vec![held[4].clone(), "absent".to_string()],
        ),
        (
            "the same held key named twice",
            vec![held[0].clone(), held[0].clone()],
        ),
        (
            "a key of the punctuation the class admits",
            vec![held[3].clone()],
        ),
        ("a key carrying a path separator", vec![held[2].clone()]),
        ("every key the resource holds, reversed", every_key_reversed),
        ("the per-call maximum, one of its keys held", full_call),
    ];

    for (index, (case, keys)) in rows.into_iter().enumerate() {
        // A campaign name identifies a campaign within its Connect instance, so each
        // row removes from a campaign of its own rather than from the last row's.
        let name = format!("untagged {index}");
        let (id, arn) = tagged_campaign(&router, region, instance, &name).await;
        // The rule, not a transcription of it: what survives is every starting tag
        // whose key the call did not name.
        let expected: BTreeMap<String, String> = starting_tags()
            .into_iter()
            .filter(|(key, _)| !keys.iter().any(|named| named == key))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();

        let path = untag_path(&arn, &keys);
        let (status, body) = call(&router, region, "DELETE", &path, &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource expected 200, got {status} {body}"
        );

        let stored = stored_tags(&router, region, &arn, case).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the resource to hold {expected:?}, got {stored:?}"
        );
        // One map per resource: the other reader projects the same record.
        let described = described_tags(&router, region, &id, case).await;
        assert_eq!(
            described, expected,
            "case {case:?}: DescribeCampaign disagreed with ListTagsForResource, got \
             {described:?}"
        );

        // Idempotent: the identical call again removes nothing further and still answers.
        let (status, body) = call(&router, region, "DELETE", &path, &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated UntagResource expected 200, got {status} {body}"
        );
        let repeated = stored_tags(&router, region, &arn, case).await;
        assert_eq!(
            repeated, expected,
            "case {case:?}: the repeated call changed the map to {repeated:?}"
        );
    }
}

/// A removal the service refuses removes NOTHING, and the refusal it answers depends on
/// what was wrong: a key outside the key shape is an input constraint, refused before
/// the ARN is resolved; an ARN whose resource path this service mints nothing under is a
/// bad request; a campaign ARN the signed-for region does not hold is a not-found.
///
/// The rows differ along which ARN is named (the live campaign, a resource path of
/// another service, a campaign id the store does not hold), which key is named (one the
/// shape admits, the reserved prefix its pattern excludes, one past the key's length
/// bound, an empty key) and how many keys are named (up to one past the per-call bound).
///
/// The key list's published minimum of zero has no row: `tagKeys` is `@required` and
/// bound to the query string, so a request carrying none of it names no route at all
/// rather than reaching this operation.
#[tokio::test]
async fn a_refused_untag_removes_nothing() {
    let region = "eu-west-2";
    let instance = "instance-1";
    let router = router();
    onboard(&router, region, instance).await;
    let (_, arn) = tagged_campaign(&router, region, instance, "refusal target").await;

    let unresolvable = "arn:aws:connect:eu-west-2:000000000000:instance/000000000000";
    let unknown_campaign = "arn:aws:connect-campaigns:eu-west-2:000000000000:campaign/999999999999";
    let over_long = "a".repeat(TAG_KEY_MAX_LENGTH + 1);
    let past_the_call_bound = absent_keys(MAX_TAG_KEYS_PER_CALL + 1);

    // (case, the ARN, the keys, the expected status, the expected wire error code)
    let rows: Vec<(&str, &str, Vec<String>, u16, &str)> = vec![
        (
            "the reserved prefix the key pattern excludes",
            &arn,
            vec!["aws:reserved".to_string()],
            400,
            "ValidationException",
        ),
        (
            "a key one character past the key length bound",
            &arn,
            vec![over_long.clone()],
            400,
            "ValidationException",
        ),
        (
            "an empty key",
            &arn,
            vec![String::new()],
            400,
            "ValidationException",
        ),
        (
            "a character the key pattern admits beside one it does not",
            &arn,
            vec!["Environment".to_string(), "has space".to_string()],
            400,
            "ValidationException",
        ),
        (
            "one key past the per-call bound",
            &arn,
            past_the_call_bound,
            400,
            "ValidationException",
        ),
        (
            "another service's resource path, with a key the shape admits",
            unresolvable,
            vec!["Environment".to_string()],
            400,
            "BadRequestException",
        ),
        (
            "another service's resource path, with a reserved key",
            unresolvable,
            vec!["aws:reserved".to_string()],
            400,
            "ValidationException",
        ),
        (
            "a campaign id the store does not hold",
            unknown_campaign,
            vec!["Environment".to_string()],
            404,
            "NotFoundException",
        ),
        (
            "a campaign id the store does not hold, with a reserved key",
            unknown_campaign,
            vec!["aws:reserved".to_string()],
            400,
            "ValidationException",
        ),
    ];

    let intact: BTreeMap<String, String> = starting_tags()
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

    for (case, target, keys, expected_status, expected_code) in rows {
        let (status, body) = call(
            &router,
            region,
            "DELETE",
            &untag_path(target, &keys),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with(expected_code),
            "case {case:?}: expected {expected_code}, got {} {body}",
            error_code(&body)
        );
        let stored = stored_tags(&router, region, &arn, case).await;
        assert_eq!(
            stored, intact,
            "case {case:?}: a refused removal changed the map to {stored:?}"
        );
    }
}

/// A campaign is reachable only through the endpoint of the region it was created in,
/// so a removal signed for another region resolves to nothing: it is refused as a
/// not-found and the campaign keeps every tag.
#[tokio::test]
async fn untag_resolves_only_within_the_region_it_was_signed_for() {
    let home = "eu-west-2";
    let instance = "instance-1";
    let router = router();
    onboard(&router, home, instance).await;
    let (_, arn) = tagged_campaign(&router, home, instance, "region scoped").await;

    let intact: BTreeMap<String, String> = starting_tags()
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

    for other in ["us-east-1", "ap-southeast-2"] {
        let keys = vec!["Environment".to_string()];
        let path = untag_path(&arn, &keys);
        let (status, body) = call(&router, other, "DELETE", &path, &Value::Null).await;
        assert_eq!(
            status, 404,
            "case {other:?}: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("NotFoundException"),
            "case {other:?}: expected NotFoundException, got {} {body}",
            error_code(&body)
        );
        let stored = stored_tags(&router, home, &arn, other).await;
        assert_eq!(
            stored, intact,
            "case {other:?}: a cross-region removal changed the map to {stored:?}"
        );
    }
}
