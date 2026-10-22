// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise resource-tagging slice — `TagResource`,
//! `UntagResource` and the `ListTagsForResource` read they are proved through —
//! driven over HTTP through the same `MultiServiceRouter` the emulator boots
//! with, so every assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: `TagResource` upserts the request's key-value
//! pairs onto the resource its `resourceArn` names and persists them in the one
//! shared store, so `ListTagsForResource` reports exactly the accumulated set
//! for every kind of resource the store holds; `UntagResource` removes exactly
//! the keys its `tagKeys` names from that same set, leaving every other key and
//! value untouched, and is idempotent for a key the resource does not hold; a
//! call that would leave a resource above the modelled per-resource tag ceiling
//! is refused with `TooManyTagsException` and stores nothing; and an ARN that
//! names no stored resource is a modelled refusal rather than a tag set
//! invented for it.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    MAX_TAGS_PER_RESOURCE, TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH,
};

// The crate's test harness lives in `asset_lifecycle`; this suite reaches those
// helpers rather than growing a second copy of them.
use super::asset_lifecycle::{call, error_code, router, REGION};

/// A `tags` request/response map built from pairs.
fn tags_json(pairs: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// The same pairs as a comparable map.
fn expected(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// One tag set under test: the `(key, value)` pairs of one `TagResource` call.
type TagRows = Vec<(String, String)>;

/// One row of the persistence table: a case label, the kind of resource the ARN
/// names, and the tags written to it.
type TaggingCase = (&'static str, &'static str, TagRows);

/// One row of the ceiling table: a case label and the successive batches that
/// take the resource to the ceiling.
type CeilingCase = (&'static str, Vec<TagRows>);

/// `n` distinct pairs, `key000`/`value000` upwards.
fn generated_pairs(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|at| (format!("key{at:03}"), format!("value{at:03}")))
        .collect()
}

/// Owned `(key, value)` pairs from `&str` literals.
fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Create an asset model carrying `tags` and return its ARN. The harness's own
/// `create_model` answers an id and takes a property list; this suite needs the
/// ARN and the create-time `tags` member instead.
async fn tagged_model(router: &MultiServiceRouter, name: &str, tags: Option<Value>) -> String {
    let mut body = json!({ "assetModelName": name });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let (status, created) = call(router, "POST", "/asset-models", Some(body)).await;
    assert_eq!(status, 202, "create asset model {name}: got {status} {created}");
    created["assetModelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("create asset model {name}: no assetModelArn in {created}"))
        .to_string()
}

/// Create an asset model and return its id (for the ARN forms below).
async fn model_id(router: &MultiServiceRouter, name: &str) -> String {
    let arn = tagged_model(router, name, None).await;
    arn.rsplit('/').next().expect("an id tail").to_string()
}

/// Create an asset from a fresh asset model and return the asset's ARN.
async fn tagged_asset(router: &MultiServiceRouter, name: &str, tags: Option<Value>) -> String {
    let model_id = model_id(router, &format!("{name}-model")).await;
    let mut body = json!({ "assetName": name, "assetModelId": model_id });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let (status, created) = call(router, "POST", "/assets", Some(body)).await;
    assert_eq!(status, 202, "create asset {name}: got {status} {created}");
    created["assetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("create asset {name}: no assetArn in {created}"))
        .to_string()
}

/// Create a gateway carrying `tags` and return its ARN.
///
/// A gateway is the third kind of record this store holds that carries a tag set
/// (`GatewayRecord.tags`, written by `CreateGateway`), so the tagging operations
/// must reach it exactly as they reach an asset or an asset model. One platform
/// device hosts at most one gateway, so each call names its own core device.
async fn tagged_gateway(router: &MultiServiceRouter, name: &str, tags: Option<Value>) -> String {
    let mut body = json!({
        "gatewayName": name,
        "gatewayPlatform": {
            "greengrassV2": {
                "coreDeviceThingName": format!("{name}-core-device"),
                "coreDeviceOperatingSystem": "LINUX_AMD64"
            }
        },
        "gatewayVersion": "3"
    });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let (status, created) = call(router, "POST", "/20200301/gateways", Some(body)).await;
    assert_eq!(status, 201, "create gateway {name}: got {status} {created}");
    created["gatewayArn"]
        .as_str()
        .unwrap_or_else(|| panic!("create gateway {name}: no gatewayArn in {created}"))
        .to_string()
}

/// A fresh resource of `kind` (`"asset-model"`, `"asset"` or `"gateway"`), by its ARN.
async fn create_resource(router: &MultiServiceRouter, kind: &str, name: &str) -> String {
    match kind {
        "asset-model" => tagged_model(router, name, None).await,
        "asset" => tagged_asset(router, name, None).await,
        "gateway" => tagged_gateway(router, name, None).await,
        other => panic!("unknown resource kind {other:?}"),
    }
}

/// `TagResource` on `arn` with `tags`.
async fn tag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    tags: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags?resourceArn={arn}"),
        Some(json!({ "tags": tags })),
    )
    .await
}

/// `ListTagsForResource` on `arn`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?resourceArn={arn}"), None).await
}

/// Percent-encode one query value the way an SDK does: everything outside RFC
/// 3986's unreserved set is encoded, so a key carrying a space or a multi-byte
/// character survives the server's decode.
fn encode_query(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => ch.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect(),
        })
        .collect()
}

/// `UntagResource` on `arn`, naming `keys`.
///
/// `DELETE /tags?resourceArn=…&tagKeys=…`: both members are `@httpQuery`, and
/// `tagKeys` is a list, so each key is its own repeated `tagKeys=` parameter.
async fn untag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    keys: &[String],
) -> (u16, Value) {
    let mut query = format!("resourceArn={}", encode_query(arn));
    for key in keys {
        query.push_str("&tagKeys=");
        query.push_str(&encode_query(key));
    }
    call(router, "DELETE", &format!("/tags?{query}"), None).await
}

/// The keys of `pairs`, as `untag_resource` takes them.
fn keys_of(pairs: &[(String, String)]) -> Vec<String> {
    pairs.iter().map(|(key, _)| key.clone()).collect()
}

/// Owned keys from `&str` literals.
fn keys(rows: &[&str]) -> Vec<String> {
    rows.iter().map(|key| (*key).to_string()).collect()
}

/// Seed `arn` with `rows` through `TagResource`, asserting the write landed.
async fn seed_tags(router: &MultiServiceRouter, arn: &str, rows: &[(String, String)], case: &str) {
    if rows.is_empty() {
        return;
    }
    let (status, body) = tag_resource(router, arn, tags_json(rows)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: seeding {} tags must answer 200, got {status} {body}",
        rows.len()
    );
}

/// The tag set `ListTagsForResource` reports for `arn`, asserting a 200.
async fn stored_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource must answer 200, got {status} {body}"
    );
    let map = body
        .get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags map in {body}"));
    map.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

// =============================================================================
// 1. A tag written on any taggable resource reads back exactly
// =============================================================================

#[tokio::test]
async fn every_tagged_resource_reads_back_exactly_the_tags_written_to_it() {
    let router = router();

    // Rows differ along what the rule generalises over: the KIND of resource the
    // ARN names, the NUMBER of pairs, and the FORM of key and value — including
    // the bounds the model declares (`TagKey` len 1..128, `TagValue` len 0..256)
    // and a multi-byte value, since `@length` counts characters.
    let long_key = "k".repeat(TAG_KEY_MAX_LENGTH);
    let short_key = "k".repeat(TAG_KEY_MIN_LENGTH);
    let long_value = "v".repeat(256);
    let cases: Vec<TaggingCase> = vec![
        (
            "asset-model, one pair",
            "asset-model",
            pairs(&[("Stage", "production")]),
        ),
        (
            "asset, three pairs",
            "asset",
            pairs(&[("Stage", "test"), ("Owner", "plant-1"), ("Line", "a")]),
        ),
        (
            "asset-model, empty value at TagValue's minimum length",
            "asset-model",
            pairs(&[("Empty", "")]),
        ),
        (
            "asset, multi-byte key and value",
            "asset",
            pairs(&[("温度", "25 °C — 施設")]),
        ),
        (
            "asset-model, key and value at their maximum lengths",
            "asset-model",
            vec![
                (long_key.clone(), long_value.clone()),
                (short_key.clone(), "one".to_string()),
            ],
        ),
        (
            "asset, the whole per-resource ceiling in one call",
            "asset",
            generated_pairs(MAX_TAGS_PER_RESOURCE),
        ),
        (
            "gateway, two pairs",
            "gateway",
            pairs(&[("Stage", "production"), ("Site", "north")]),
        ),
        (
            "gateway, the whole per-resource ceiling in one call",
            "gateway",
            generated_pairs(MAX_TAGS_PER_RESOURCE),
        ),
    ];

    for (at, (case, kind, rows)) in cases.iter().enumerate() {
        let arn = create_resource(&router, kind, &format!("res-{at}")).await;

        // Before the write the resource exists and carries no tag: an empty map,
        // which is a different answer from the refusal an unknown ARN gets.
        let before = stored_tags(&router, &arn, case).await;
        assert!(
            before.is_empty(),
            "case {case:?}: an untagged resource must report an empty tag set, got {before:?}"
        );

        let (status, body) = tag_resource(&router, &arn, tags_json(rows)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must answer 200, got {status} {body}"
        );

        let got = stored_tags(&router, &arn, case).await;
        assert_eq!(
            got,
            expected(rows),
            "case {case:?}: the listed tags must be exactly the ones written, got {got:?}"
        );

        // The read is a report, not a mutation: a second read answers the same.
        let again = stored_tags(&router, &arn, case).await;
        assert_eq!(
            again, got,
            "case {case:?}: reading the tags must not change what a later caller sees, \
             got {again:?} then {got:?}"
        );
    }
}

// =============================================================================
// 2. Tags set at create time are the same tag set this operation reads
// =============================================================================

#[tokio::test]
async fn create_time_tags_are_visible_to_the_tag_reader_and_writer() {
    let router = router();

    // One store, one tag set per resource: whichever operation wrote a tag, the
    // reader reports it and `TagResource` merges into it.
    let seeded = pairs(&[("Seeded", "at-create"), ("Stage", "test")]);
    let cases = [
        ("asset model created with tags", "asset-model"),
        ("asset created with tags", "asset"),
        ("gateway created with tags", "gateway"),
    ];
    for (case, kind) in cases {
        let arn = match kind {
            "asset-model" => tagged_model(&router, "seeded-model", Some(tags_json(&seeded))).await,
            "gateway" => tagged_gateway(&router, "seeded-gateway", Some(tags_json(&seeded))).await,
            _ => tagged_asset(&router, "seeded-asset", Some(tags_json(&seeded))).await,
        };

        let got = stored_tags(&router, &arn, case).await;
        assert_eq!(
            got,
            expected(&seeded),
            "case {case:?}: the create-time tags must be the ones listed, got {got:?}"
        );

        // An upsert over them: one key edited, one added, one untouched.
        let (status, body) =
            tag_resource(&router, &arn, json!({ "Stage": "production", "Owner": "ops" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must answer 200, got {status} {body}"
        );
        let got = stored_tags(&router, &arn, case).await;
        let want = expected(&pairs(&[
            ("Seeded", "at-create"),
            ("Stage", "production"),
            ("Owner", "ops"),
        ]));
        assert_eq!(
            got, want,
            "case {case:?}: an edited key takes the new value, a new key is added and an \
             unnamed key is kept, got {got:?}"
        );
    }
}

// =============================================================================
// 3. The per-resource ceiling is reported, never exceeded
// =============================================================================

#[tokio::test]
async fn the_per_resource_tag_ceiling_is_reported_and_never_exceeded() {
    let router = router();

    // Rows differ in HOW the ceiling is reached: in one call, or across calls
    // whose sizes sum past it.
    let cases: Vec<CeilingCase> = vec![
        (
            "the whole ceiling in one call, then one more key",
            vec![generated_pairs(MAX_TAGS_PER_RESOURCE)],
        ),
        (
            "two calls that together reach the ceiling, then one more key",
            vec![
                generated_pairs(MAX_TAGS_PER_RESOURCE)[..20].to_vec(),
                generated_pairs(MAX_TAGS_PER_RESOURCE)[20..].to_vec(),
            ],
        ),
    ];

    for (case, batches) in &cases {
        let arn = create_resource(&router, "asset-model", "capped").await;
        for batch in batches {
            let (status, body) = tag_resource(&router, &arn, tags_json(batch)).await;
            assert_eq!(
                status,
                200,
                "case {case:?}: a batch of {} tags at or below the ceiling must be accepted, \
                 got {status} {body}",
                batch.len()
            );
        }
        let at_cap = stored_tags(&router, &arn, case).await;
        assert_eq!(
            at_cap.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: the resource must carry the whole ceiling, got {}",
            at_cap.len()
        );

        // At the ceiling, re-tagging a key already held does not grow the set.
        let (status, body) = tag_resource(&router, &arn, json!({ "key000": "changed" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: replacing a held key at the ceiling must be accepted, \
             got {status} {body}"
        );
        let replaced = stored_tags(&router, &arn, case).await;
        assert_eq!(
            replaced.get("key000").map(String::as_str),
            Some("changed"),
            "case {case:?}: the replacement value must be stored, got {:?}",
            replaced.get("key000")
        );
        assert_eq!(
            replaced.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: replacing a key must leave the count at the ceiling, got {}",
            replaced.len()
        );

        // One key past the ceiling: refused, and nothing stored.
        let (status, body) = tag_resource(&router, &arn, json!({ "overflow": "x" })).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "TooManyTagsException"),
            "case {case:?}: one key past the ceiling must be refused, got {status} {body}"
        );
        assert_eq!(
            body.get("resourceName").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the refusal must name the resource it was about, got {body}"
        );
        let after = stored_tags(&router, &arn, case).await;
        assert_eq!(
            after, replaced,
            "case {case:?}: a refused call must store nothing, got {after:?}"
        );
    }

    // And a batch that is itself larger than the ceiling is refused the same way,
    // on a resource that holds no tag at all. Only the STATUS is asserted here:
    // `TagMap` carries `@length(max: 50)`, so a 51-entry map is refused by the
    // boundary validator before this handler runs, and the error CODE that walk
    // renders is the subject of its own issue ("tag-count constraint violation
    // answered with an unmodeled ValidationException").
    let arn = create_resource(&router, "asset", "capped-at-once").await;
    let over = generated_pairs(MAX_TAGS_PER_RESOURCE + 1);
    let (status, body) = tag_resource(&router, &arn, tags_json(&over)).await;
    assert_eq!(
        status, 400,
        "case {:?}: {} tags in one call must be refused, got {status} {body}",
        "one batch past the ceiling",
        over.len()
    );
    let after = stored_tags(&router, &arn, "one batch past the ceiling").await;
    assert!(
        after.is_empty(),
        "case {:?}: a refused call must store nothing, got {after:?}",
        "one batch past the ceiling"
    );
}

// =============================================================================
// 4. An ARN that names no stored resource is a modelled refusal
// =============================================================================

#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_refused_by_both_operations() {
    let router = router();
    let account = engine::EMULATOR_ACCOUNT_ID;
    // A real resource, so the rows below differ from it in exactly one segment.
    let model_arn = tagged_model(&router, "present", None).await;
    let model_id = model_arn.rsplit('/').next().expect("an id tail").to_string();
    let unknown_id = "11111111-2222-3333-4444-555555555555";

    // Rows differ in WHICH part of the ARN names nothing: the id, the resource
    // type, the region, the account — each a well-formed IoT SiteWise ARN, so
    // each is "the requested resource can't be found" (404).
    let not_found = [
        (
            "an asset-model id the store does not hold",
            format!("arn:aws:iotsitewise:{REGION}:{account}:asset-model/{unknown_id}"),
        ),
        (
            "an asset id the store does not hold",
            format!("arn:aws:iotsitewise:{REGION}:{account}:asset/{unknown_id}"),
        ),
        (
            "a gateway id the store does not hold",
            format!("arn:aws:iotsitewise:{REGION}:{account}:gateway/{unknown_id}"),
        ),
        (
            "a resource type this emulator holds none of",
            format!("arn:aws:iotsitewise:{REGION}:{account}:portal/{unknown_id}"),
        ),
        (
            "the right id in another region",
            format!("arn:aws:iotsitewise:eu-west-1:{account}:asset-model/{model_id}"),
        ),
        (
            "the right id in another account",
            format!("arn:aws:iotsitewise:{REGION}:111111111111:asset-model/{model_id}"),
        ),
        (
            "the right id in another partition",
            format!("arn:aws-cn:iotsitewise:{REGION}:{account}:asset-model/{model_id}"),
        ),
    ];
    for (case, arn) in &not_found {
        let (status, body) = tag_resource(&router, arn, json!({ "Stage": "test" })).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: TagResource must refuse {arn:?} as not found, got {status} {body}"
        );
        let (status, body) = list_tags(&router, arn).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: ListTagsForResource must refuse {arn:?} as not found, \
             got {status} {body}"
        );
    }

    // Rows that are not an ARN of THIS service at all: no IoT SiteWise resource
    // could ever answer for them, so they are a malformed request (400).
    let invalid = [
        ("not an ARN at all", "my-asset-model".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:s3:::bucket/{unknown_id}"),
        ),
        (
            "an IoT SiteWise ARN with no resource id",
            format!("arn:aws:iotsitewise:{REGION}:{account}:asset-model"),
        ),
        (
            "an ARN missing its trailing resource section",
            format!("arn:aws:iotsitewise:{REGION}:{account}"),
        ),
    ];
    for (case, arn) in &invalid {
        let (status, body) = tag_resource(&router, arn, json!({ "Stage": "test" })).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidRequestException"),
            "case {case:?}: TagResource must refuse {arn:?} as invalid, got {status} {body}"
        );
        let (status, body) = list_tags(&router, arn).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidRequestException"),
            "case {case:?}: ListTagsForResource must refuse {arn:?} as invalid, \
             got {status} {body}"
        );
    }

    // The present resource still answers, so nothing above touched the store.
    let got = stored_tags(&router, &model_arn, "the present resource").await;
    assert!(
        got.is_empty(),
        "the refusals above must have stored nothing on the real resource, got {got:?}"
    );
}

// =============================================================================
// 5. A tag key outside the modelled TagKey bound is refused whole
// =============================================================================

#[tokio::test]
async fn a_tag_key_outside_the_modelled_bound_is_refused_and_stores_nothing() {
    let router = router();
    let arn = tagged_model(&router, "key-bounds", None).await;

    // The published bound is `TagKey`'s own `@length(min: 1, max: 128)`, read from
    // this service's Smithy model (`TagMap`'s key target). Each bound is tested at
    // its edge and one past it.
    let at_min = "k".repeat(TAG_KEY_MIN_LENGTH);
    let at_max = "k".repeat(TAG_KEY_MAX_LENGTH);
    let accepted = [("the shortest legal key", at_min), ("the longest legal key", at_max)];
    for (case, key) in &accepted {
        let (status, body) = tag_resource(&router, &arn, json!({ key.clone(): "v" })).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: a key of {} characters must be accepted, got {status} {body}",
            key.chars().count()
        );
    }
    let held = stored_tags(&router, &arn, "the keys at both bounds").await;
    assert_eq!(
        held.len(),
        accepted.len(),
        "both legal keys must be stored, got {held:?}"
    );

    let refused = [
        (
            "one character past the maximum",
            "k".repeat(TAG_KEY_MAX_LENGTH + 1),
        ),
        ("one character short of the minimum", String::new()),
    ];
    for (case, key) in &refused {
        // The batch also carries a legal pair, so a refusal that stored the legal
        // half of it would be visible below.
        let (status, body) = tag_resource(
            &router,
            &arn,
            json!({ key.clone(): "v", "Legal": "yes" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidRequestException"),
            "case {case:?}: a key of {} characters must be refused, got {status} {body}",
            key.chars().count()
        );
        let after = stored_tags(&router, &arn, case).await;
        assert_eq!(
            after, held,
            "case {case:?}: a refused call must store no part of its batch, got {after:?}"
        );
    }
}

// =============================================================================
// 6. UntagResource removes exactly the named keys and nothing else
// =============================================================================

/// One row of the untag table: a case label, the kind of resource, the tags it
/// is seeded with, the keys the `UntagResource` call names, and the pairs that
/// must remain afterwards.
type UntagCase = (&'static str, &'static str, TagRows, Vec<String>, TagRows);

/// How many keys one `UntagResource` call may name.
///
/// The model's own bound, from this service's Smithy model: `tagKeys` targets
/// `TagKeyList`, declared `@length(min: 0, max: 200)`.
const MAX_TAG_KEYS_PER_CALL: usize = 200;

#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys_and_is_idempotent() {
    let router = router();

    let long_key = "k".repeat(TAG_KEY_MAX_LENGTH);
    let short_key = "k".repeat(TAG_KEY_MIN_LENGTH);
    let ceiling = generated_pairs(MAX_TAGS_PER_RESOURCE);

    // Rows differ along what the rule generalises over: the KIND of resource the
    // ARN names, HOW MANY of its keys the call names (none of them, one, some,
    // all), whether a named key is one the resource holds, the FORM of the key
    // (multi-byte, at `TagKey`'s minimum and maximum length), repeated entries
    // in the list, and the size of the list itself.
    let cases: Vec<UntagCase> = vec![
        (
            "asset-model, one of three keys",
            "asset-model",
            pairs(&[("Stage", "production"), ("Owner", "plant-1"), ("Line", "a")]),
            keys(&["Owner"]),
            pairs(&[("Stage", "production"), ("Line", "a")]),
        ),
        (
            "asset, two of three keys",
            "asset",
            pairs(&[("Stage", "production"), ("Owner", "plant-1"), ("Line", "a")]),
            keys(&["Stage", "Line"]),
            pairs(&[("Owner", "plant-1")]),
        ),
        (
            "gateway, every key it holds",
            "gateway",
            pairs(&[("Stage", "production"), ("Site", "north")]),
            keys(&["Stage", "Site"]),
            Vec::new(),
        ),
        (
            "asset-model, a key the resource does not hold",
            "asset-model",
            pairs(&[("Stage", "production"), ("Owner", "plant-1")]),
            keys(&["Absent"]),
            pairs(&[("Stage", "production"), ("Owner", "plant-1")]),
        ),
        (
            "asset, one held key beside one it does not hold",
            "asset",
            pairs(&[("Stage", "production"), ("Owner", "plant-1")]),
            keys(&["Stage", "Absent"]),
            pairs(&[("Owner", "plant-1")]),
        ),
        (
            "asset, a multi-byte key",
            "asset",
            pairs(&[("温度", "25 °C — 施設"), ("Stage", "test")]),
            keys(&["温度"]),
            pairs(&[("Stage", "test")]),
        ),
        (
            "asset-model, the key at TagKey's maximum length",
            "asset-model",
            vec![
                (long_key.clone(), "long".to_string()),
                (short_key.clone(), "short".to_string()),
            ],
            vec![long_key.clone()],
            vec![(short_key.clone(), "short".to_string())],
        ),
        (
            "gateway, the key at TagKey's minimum length",
            "gateway",
            vec![
                (long_key.clone(), "long".to_string()),
                (short_key.clone(), "short".to_string()),
            ],
            vec![short_key.clone()],
            vec![(long_key.clone(), "long".to_string())],
        ),
        (
            "asset, a key named twice in one call",
            "asset",
            pairs(&[("Stage", "production"), ("Owner", "plant-1")]),
            keys(&["Stage", "Stage"]),
            pairs(&[("Owner", "plant-1")]),
        ),
        (
            "asset-model, all but one of the whole per-resource ceiling",
            "asset-model",
            ceiling.clone(),
            keys_of(&ceiling[1..]),
            ceiling[..1].to_vec(),
        ),
        (
            "asset, the whole list bound, two of whose keys are held",
            "asset",
            pairs(&[("Stage", "production"), ("Owner", "plant-1")]),
            {
                let mut named = keys(&["Stage"]);
                named.extend(
                    (1..MAX_TAG_KEYS_PER_CALL).map(|at| format!("absent{at:03}")),
                );
                named
            },
            pairs(&[("Owner", "plant-1")]),
        ),
    ];

    for (at, (case, kind, seeded, named, remaining)) in cases.iter().enumerate() {
        assert!(
            named.len() <= MAX_TAG_KEYS_PER_CALL,
            "case {case:?}: the row itself must stay within the modelled list bound, got {}",
            named.len()
        );
        let arn = create_resource(&router, kind, &format!("untag-{at}")).await;
        seed_tags(&router, &arn, seeded, case).await;

        let (status, body) = untag_resource(&router, &arn, named).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource must answer 200, got {status} {body}"
        );

        let got = stored_tags(&router, &arn, case).await;
        assert_eq!(
            got,
            expected(remaining),
            "case {case:?}: exactly the named keys must be gone and every other pair \
             kept, got {got:?}"
        );

        // Removal is idempotent: the same call again is accepted and leaves the
        // same tag set, so a key the resource no longer holds is not an error.
        let (status, body) = untag_resource(&router, &arn, named).await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating the call must answer 200, got {status} {body}"
        );
        let again = stored_tags(&router, &arn, case).await;
        assert_eq!(
            again, got,
            "case {case:?}: repeating the call must leave the same tag set, got {again:?}"
        );

        // A removed key is removable again by a later `TagResource`: the removal
        // took the key out of the set rather than marking it unusable.
        if let Some((key, value)) = seeded.iter().find(|(key, _)| named.contains(key)) {
            let (status, body) =
                tag_resource(&router, &arn, tags_json(&[(key.clone(), value.clone())])).await;
            assert_eq!(
                status, 200,
                "case {case:?}: re-tagging a removed key must answer 200, got {status} {body}"
            );
            let retagged = stored_tags(&router, &arn, case).await;
            assert_eq!(
                retagged.get(key).map(String::as_str),
                Some(value.as_str()),
                "case {case:?}: the re-tagged key must read back, got {:?}",
                retagged.get(key)
            );
        }
    }
}

// =============================================================================
// 7. UntagResource refuses an ARN that names no stored resource
// =============================================================================

#[tokio::test]
async fn untag_resource_refuses_an_arn_that_names_no_stored_resource() {
    let router = router();
    let account = engine::EMULATOR_ACCOUNT_ID;
    let held = pairs(&[("Stage", "production"), ("Owner", "plant-1")]);
    let model_arn = tagged_model(&router, "untag-present", Some(tags_json(&held))).await;
    let model_id = model_arn.rsplit('/').next().expect("an id tail").to_string();
    let unknown_id = "11111111-2222-3333-4444-555555555555";

    // Rows differ in WHICH part of the ARN names nothing — each is a well-formed
    // IoT SiteWise ARN, so each is "the requested resource can't be found" (404).
    let not_found = [
        (
            "an asset-model id the store does not hold",
            format!("arn:aws:iotsitewise:{REGION}:{account}:asset-model/{unknown_id}"),
        ),
        (
            "an asset id the store does not hold",
            format!("arn:aws:iotsitewise:{REGION}:{account}:asset/{unknown_id}"),
        ),
        (
            "a gateway id the store does not hold",
            format!("arn:aws:iotsitewise:{REGION}:{account}:gateway/{unknown_id}"),
        ),
        (
            "a resource type this emulator holds none of",
            format!("arn:aws:iotsitewise:{REGION}:{account}:portal/{unknown_id}"),
        ),
        (
            "the right id in another region",
            format!("arn:aws:iotsitewise:eu-west-1:{account}:asset-model/{model_id}"),
        ),
        (
            "the right id in another account",
            format!("arn:aws:iotsitewise:{REGION}:111111111111:asset-model/{model_id}"),
        ),
        (
            "the right id in another partition",
            format!("arn:aws-cn:iotsitewise:{REGION}:{account}:asset-model/{model_id}"),
        ),
    ];
    for (case, arn) in &not_found {
        let (status, body) = untag_resource(&router, arn, &keys(&["Stage"])).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: UntagResource must refuse {arn:?} as not found, got {status} {body}"
        );
    }

    // Rows that are not an ARN of THIS service at all: no IoT SiteWise resource
    // could ever answer for them, so they are a malformed request (400).
    let invalid = [
        ("not an ARN at all", "my-asset-model".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:s3:::bucket/{unknown_id}"),
        ),
        (
            "an IoT SiteWise ARN with no resource id",
            format!("arn:aws:iotsitewise:{REGION}:{account}:asset-model"),
        ),
        (
            "an ARN missing its trailing resource section",
            format!("arn:aws:iotsitewise:{REGION}:{account}"),
        ),
    ];
    for (case, arn) in &invalid {
        let (status, body) = untag_resource(&router, arn, &keys(&["Stage"])).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidRequestException"),
            "case {case:?}: UntagResource must refuse {arn:?} as invalid, got {status} {body}"
        );
    }

    // Every refusal above left the real resource's tags exactly as they were.
    let got = stored_tags(&router, &model_arn, "the present resource").await;
    assert_eq!(
        got,
        expected(&held),
        "the refusals above must have removed nothing from the real resource, got {got:?}"
    );
}

// =============================================================================
// 8. A tagKeys list outside the modelled bounds removes nothing
// =============================================================================

#[tokio::test]
async fn a_tag_keys_list_outside_the_modelled_bounds_removes_nothing() {
    let router = router();
    let held = pairs(&[("Stage", "production"), ("Owner", "plant-1")]);
    let arn = tagged_model(&router, "untag-bounds", Some(tags_json(&held))).await;

    // The published bounds are `TagKeyList`'s own `@length(min: 0, max: 200)` and
    // its member target `TagKey`'s `@length(min: 1, max: 128)`, both read from
    // this service's Smithy model. Each is tested one past its edge; the edges
    // themselves are rows of the table above.
    let over_the_list_bound: Vec<String> = (0..=MAX_TAG_KEYS_PER_CALL)
        .map(|at| format!("absent{at:03}"))
        .collect();
    let refused = [
        (
            "one key past the list's maximum length",
            over_the_list_bound,
        ),
        (
            "a key one character past TagKey's maximum length",
            vec!["k".repeat(TAG_KEY_MAX_LENGTH + 1)],
        ),
        (
            "a key one character short of TagKey's minimum length",
            vec![String::new()],
        ),
    ];
    for (case, named) in &refused {
        // Each list also names a key the resource DOES hold, so a refusal that
        // removed the legal part of it would be visible below.
        let mut with_held = named.clone();
        with_held.push("Stage".to_string());
        let (status, body) = untag_resource(&router, &arn, &with_held).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a list of {} keys must be refused, got {status} {body}",
            with_held.len()
        );
        let after = stored_tags(&router, &arn, case).await;
        assert_eq!(
            after,
            expected(&held),
            "case {case:?}: a refused call must remove no part of its list, got {after:?}"
        );
    }
}
